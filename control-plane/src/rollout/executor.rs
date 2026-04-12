use crate::db::Db;
use crate::state::FleetState;
use anyhow::Context;
use nixfleet_types::metrics as m;
use nixfleet_types::DesiredGeneration;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Parse a failure threshold spec into an absolute count.
///
/// - `"3"` → 3 (absolute)
/// - `"30%"` → ceil(batch_size * 0.30)
///
/// Returns an error on malformed input (e.g. "foo%"). Previously the
/// parser silently coerced invalid spec strings to safe defaults, which
/// masked operator misconfiguration.
pub fn parse_threshold(spec: &str, batch_size: usize) -> anyhow::Result<usize> {
    if let Some(pct_str) = spec.strip_suffix('%') {
        let pct: f64 = pct_str
            .parse()
            .with_context(|| format!("invalid percentage in failure threshold: {spec:?}"))?;
        if !(0.0..=100.0).contains(&pct) {
            anyhow::bail!("failure threshold percentage out of range [0, 100]: {spec:?}");
        }
        Ok((batch_size as f64 * pct / 100.0).ceil() as usize)
    } else {
        spec.parse::<usize>()
            .with_context(|| format!("invalid absolute failure threshold: {spec:?}"))
    }
}

/// Log an error from an audit/event insertion without aborting the
/// caller. Rollout events and audit events are diagnostic and should
/// never silently disappear, but an insert failure also should not
/// crash an in-flight rollout. We surface the error to operators.
fn log_event_err(kind: &str, result: anyhow::Result<()>) {
    if let Err(e) = result {
        tracing::warn!(event = kind, error = %e, "failed to record rollout event");
    }
}

/// Spawn the executor background task. Returns a join handle.
pub fn spawn(state: Arc<RwLock<FleetState>>, db: Arc<Db>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            if let Err(error) = tick(&state, &db).await {
                tracing::error!(%error, "Rollout executor tick failed");
            }
        }
    })
}

/// One evaluation cycle: advance all running rollouts.
async fn tick(state: &Arc<RwLock<FleetState>>, db: &Arc<Db>) -> anyhow::Result<()> {
    let rollouts = db.list_rollouts_by_status(Some("running"), 100)?;

    for rollout in rollouts {
        if let Err(error) = process_rollout(state, db, &rollout).await {
            tracing::error!(rollout_id = %rollout.id, %error, "Failed to process rollout");
        }
    }

    Ok(())
}

async fn process_rollout(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
) -> anyhow::Result<()> {
    let batches = db.get_rollout_batches(&rollout.id)?;

    // Find the current batch: first batch that is pending, deploying, or waiting_health
    let current_batch = batches
        .iter()
        .find(|b| b.status == "pending" || b.status == "deploying" || b.status == "waiting_health");

    let Some(batch) = current_batch else {
        // No active batch — check if all batches succeeded
        let all_succeeded = batches.iter().all(|b| b.status == "succeeded");
        if all_succeeded && !batches.is_empty() {
            db.update_rollout_status(&rollout.id, "completed")?;
            log_event_err(
                "status_change",
                db.insert_rollout_event(
                    &rollout.id,
                    "status_change",
                    "{\"from\":\"running\",\"to\":\"completed\"}",
                    "executor",
                ),
            );
            db.insert_audit_event(
                "executor",
                "rollout.completed",
                &rollout.id,
                Some("All batches succeeded"),
            )?;
            tracing::info!(rollout_id = %rollout.id, "Rollout completed");
            metrics::counter!(m::ROLLOUTS_TOTAL, "status" => "completed").increment(1);
            update_rollouts_active_gauge(db);
        }
        return Ok(());
    };

    let machine_ids: Vec<String> = serde_json::from_str(&batch.machine_ids)?;

    // Build entry map for generation gating in evaluate_batch
    let release_entries = db.get_release_entries(&rollout.release_id)?;
    let entry_map: std::collections::HashMap<String, String> = release_entries
        .iter()
        .map(|e| (e.hostname.clone(), e.store_path.clone()))
        .collect();

    match batch.status.as_str() {
        "pending" => deploy_batch(state, db, rollout, batch, &machine_ids).await?,
        "deploying" | "waiting_health" => {
            evaluate_batch(state, db, rollout, batch, &machine_ids, &batches, &entry_map)
                .await?;
        }
        _ => {}
    }

    Ok(())
}

/// Set desired_generation for all machines in the batch and mark it deploying.
async fn deploy_batch(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
    batch: &crate::db::RolloutBatchRow,
    machine_ids: &[String],
) -> anyhow::Result<()> {
    let release_entries = db.get_release_entries(&rollout.release_id)?;
    let entry_map: std::collections::HashMap<&str, &str> = release_entries
        .iter()
        .map(|e| (e.hostname.as_str(), e.store_path.as_str()))
        .collect();

    let cache_url = rollout.cache_url.clone();
    let mut previous_gens: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    {
        let mut fleet = state.write().await;
        for machine_id in machine_ids {
            if let Ok(Some(current_hash)) = db.get_desired_generation(machine_id) {
                previous_gens.insert(machine_id.clone(), current_hash);
            }

            let store_path = entry_map.get(machine_id.as_str()).ok_or_else(|| {
                anyhow::anyhow!("machine {} not found in release entries", machine_id)
            })?;

            db.set_desired_generation(machine_id, store_path)?;

            let machine = fleet.get_or_create(machine_id);
            machine.desired_generation = Some(DesiredGeneration {
                hash: store_path.to_string(),
                cache_url: cache_url.clone(),
                poll_hint: None,
            });
        }
    }

    let prev_json = serde_json::to_string(&previous_gens)
        .context("failed to serialize previous_generations map")?;
    db.update_batch_previous_generations(&batch.id, &prev_json)?;

    db.update_batch_status(&batch.id, "deploying")?;

    log_event_err(
        "batch_started",
        db.insert_rollout_event(
            &rollout.id,
            "batch_started",
            &format!(
                "{{\"batch_index\":{},\"machines\":{}}}",
                batch.batch_index,
                machine_ids.len()
            ),
            "executor",
        ),
    );

    db.insert_audit_event(
        "executor",
        "batch.deploying",
        &batch.id,
        Some(&format!(
            "Rollout {} batch {} deploying {} machines",
            rollout.id,
            batch.batch_index,
            machine_ids.len()
        )),
    )?;

    tracing::info!(
        rollout_id = %rollout.id,
        batch_id = %batch.id,
        batch_index = batch.batch_index,
        machines = machine_ids.len(),
        "Batch deploying"
    );

    Ok(())
}

/// Evaluate health of machines in a deploying/waiting_health batch.
///
/// Generation gating: a machine's report is only considered if its `generation`
/// matches the desired store path from the release. Reports from a previous
/// generation are treated as "pending" (the machine hasn't applied yet).
async fn evaluate_batch(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
    batch: &crate::db::RolloutBatchRow,
    machine_ids: &[String],
    all_batches: &[crate::db::RolloutBatchRow],
    entry_map: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    let started_at = batch.started_at.as_deref().unwrap_or("");
    if started_at.is_empty() {
        // No started_at timestamp yet — wait for next tick
        return Ok(());
    }

    let mut healthy_count = 0usize;
    let mut unhealthy_count = 0usize;
    let mut pending_count = 0usize;

    for machine_id in machine_ids {
        let desired_path = entry_map.get(machine_id).map(|s| s.as_str()).unwrap_or("");

        // Check the latest report to see if the machine has applied the desired generation
        let recent_reports = db.get_recent_reports(machine_id, 1)?;
        let report = recent_reports.first();

        let on_desired_gen = report
            .map(|r| r.generation == desired_path)
            .unwrap_or(false);

        if !on_desired_gen {
            // Machine hasn't applied the desired generation yet.
            // Check if it explicitly failed (report received after batch started, success=false).
            if let Some(r) = report {
                if !r.success && r.received_at.as_str() >= started_at {
                    // Explicit deployment failure on old or wrong generation
                    unhealthy_count += 1;
                } else {
                    // Still on previous generation, waiting for apply
                    pending_count += 1;
                }
            } else {
                pending_count += 1;
            }
        } else {
            // Machine is on the desired generation — evaluate health
            let health_reports = db.get_health_reports_since(machine_id, started_at)?;
            if !health_reports.is_empty() {
                if health_reports[0].all_passed {
                    healthy_count += 1;
                } else {
                    unhealthy_count += 1;
                }
            } else if let Some(r) = report {
                // Fallback to the most recent general report. MUST filter
                // by started_at — otherwise on resume, a stale unhealthy
                // report from before the failure was cleared would flip
                // the batch back to failed immediately, before the agent
                // has a chance to send a fresh healthy report. This
                // mirrors the `received_at >= started_at` filter in the
                // `!on_desired_gen` branch above.
                if r.received_at.as_str() < started_at {
                    // Stale report from before this batch's started_at
                    // (common right after resume). Treat as pending —
                    // give the agent a chance to send a fresh one.
                    pending_count += 1;
                } else if r.success {
                    // On desired gen and success, but no health report yet
                    pending_count += 1;
                } else {
                    // On desired gen but reported failure
                    unhealthy_count += 1;
                }
            } else {
                pending_count += 1;
            }
        }
    }

    // If any machines haven't reported yet, check health timeout.
    //
    // A corrupted started_at timestamp must not silently become "no
    // timeout" — that would make the batch wait forever. Surface the
    // parse error so the executor can pause the rollout explicitly.
    if pending_count > 0 {
        let batch_start =
            chrono::NaiveDateTime::parse_from_str(started_at, "%Y-%m-%d %H:%M:%S")
                .with_context(|| {
                    format!(
                        "invalid batch started_at timestamp {started_at:?} for batch {}",
                        batch.id
                    )
                })?;
        let batch_start_utc = chrono::TimeZone::from_utc_datetime(&chrono::Utc, &batch_start);
        let elapsed = chrono::Utc::now()
            .signed_duration_since(batch_start_utc)
            .num_seconds();
        let timed_out = elapsed >= rollout.health_timeout;

        if timed_out {
            // Treat pending machines as unhealthy
            unhealthy_count += pending_count;
            tracing::warn!(
                rollout_id = %rollout.id,
                batch_id = %batch.id,
                pending = pending_count,
                health_timeout = rollout.health_timeout,
                "Health timeout elapsed, treating pending machines as unhealthy"
            );
        } else {
            if batch.status == "deploying" {
                db.update_batch_status(&batch.id, "waiting_health")?;
                tracing::info!(
                    rollout_id = %rollout.id,
                    batch_id = %batch.id,
                    healthy = healthy_count,
                    unhealthy = unhealthy_count,
                    pending = pending_count,
                    "Batch waiting for health reports"
                );
            }
            return Ok(());
        }
    }

    // All machines have reported — evaluate.
    //
    // Semantic: failure_threshold = N means "allow up to N unhealthy machines
    // per batch; the (N+1)th unhealthy machine fails the batch". Therefore the
    // success condition is `unhealthy_count <= threshold`. threshold = 0 reads
    // as zero tolerance (any single failure pauses the batch).
    let threshold = parse_threshold(&rollout.failure_threshold, machine_ids.len())
        .with_context(|| format!("rollout {} has invalid failure_threshold", rollout.id))?;

    if unhealthy_count <= threshold {
        // Batch succeeded
        db.update_batch_status(&batch.id, "succeeded")?;
        log_event_err(
            "batch_completed",
            db.insert_rollout_event(
                &rollout.id,
                "batch_completed",
                &format!(
                    "{{\"batch_index\":{},\"healthy\":{healthy_count},\"unhealthy\":{unhealthy_count}}}",
                    batch.batch_index
                ),
                "executor",
            ),
        );
        db.insert_audit_event(
            "executor",
            "batch.succeeded",
            &batch.id,
            Some(&format!(
                "Batch {} succeeded: {healthy_count} healthy, {unhealthy_count} unhealthy (threshold: {threshold})",
                batch.batch_index
            )),
        )?;
        tracing::info!(
            rollout_id = %rollout.id,
            batch_id = %batch.id,
            healthy = healthy_count,
            unhealthy = unhealthy_count,
            threshold,
            "Batch succeeded"
        );
    } else {
        // Batch failed
        db.update_batch_status(&batch.id, "failed")?;
        log_event_err(
            "batch_failed",
            db.insert_rollout_event(
                &rollout.id,
                "batch_failed",
                &format!(
                    "{{\"batch_index\":{},\"unhealthy\":{unhealthy_count},\"threshold\":{threshold}}}",
                    batch.batch_index
                ),
                "executor",
            ),
        );
        db.insert_audit_event(
            "executor",
            "batch.failed",
            &batch.id,
            Some(&format!(
                "Batch {} failed: {unhealthy_count} unhealthy > threshold {threshold}",
                batch.batch_index
            )),
        )?;

        match rollout.on_failure.as_str() {
            "pause" => {
                db.update_rollout_status(&rollout.id, "paused")?;
                log_event_err(
                    "status_change",
                    db.insert_rollout_event(
                        &rollout.id,
                        "status_change",
                        "{\"from\":\"running\",\"to\":\"paused\",\"reason\":\"batch failure\"}",
                        "executor",
                    ),
                );
                db.insert_audit_event(
                    "executor",
                    "rollout.paused",
                    &rollout.id,
                    Some("Batch failure triggered pause"),
                )?;
                tracing::warn!(
                    rollout_id = %rollout.id,
                    batch_id = %batch.id,
                    "Rollout paused due to batch failure"
                );
                metrics::counter!(m::ROLLOUTS_TOTAL, "status" => "paused").increment(1);
                update_rollouts_active_gauge(db);
            }
            "revert" => {
                revert_completed_batches(state, db, rollout, all_batches).await?;
                db.update_rollout_status(&rollout.id, "failed")?;
                log_event_err(
                    "status_change",
                    db.insert_rollout_event(
                        &rollout.id,
                        "status_change",
                        "{\"from\":\"running\",\"to\":\"failed\",\"reason\":\"batch failure + revert\"}",
                        "executor",
                    ),
                );
                db.insert_audit_event(
                    "executor",
                    "rollout.failed",
                    &rollout.id,
                    Some("Batch failure triggered revert"),
                )?;
                tracing::warn!(
                    rollout_id = %rollout.id,
                    batch_id = %batch.id,
                    "Rollout failed — reverting completed batches"
                );
                metrics::counter!(m::ROLLOUTS_TOTAL, "status" => "failed").increment(1);
                update_rollouts_active_gauge(db);
            }
            _ => {
                // Default: pause
                db.update_rollout_status(&rollout.id, "paused")?;
                tracing::warn!(
                    rollout_id = %rollout.id,
                    on_failure = %rollout.on_failure,
                    "Unknown on_failure action, defaulting to pause"
                );
                metrics::counter!(m::ROLLOUTS_TOTAL, "status" => "paused").increment(1);
                update_rollouts_active_gauge(db);
            }
        }
    }

    Ok(())
}

/// Recalculate the ROLLOUTS_ACTIVE gauge from the live database state.
fn update_rollouts_active_gauge(db: &Db) {
    let active = db
        .list_rollouts_by_status(Some("running"), 1000)
        .map(|r| r.len())
        .unwrap_or(0);
    metrics::gauge!(m::ROLLOUTS_ACTIVE).set(active as f64);
}

/// Revert all machines in completed (succeeded) batches to their previous generations.
async fn revert_completed_batches(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
    all_batches: &[crate::db::RolloutBatchRow],
) -> anyhow::Result<()> {
    let mut fleet = state.write().await;

    for batch in all_batches {
        if batch.status != "succeeded" {
            continue;
        }

        // A corrupt previous_generations or machine_ids column would
        // silently skip the revert — dangerous, because the operator
        // would see "reverted" with no actual rollback. Surface the
        // parse error so the caller can abort the revert cleanly.
        let previous_gens: std::collections::HashMap<String, String> =
            serde_json::from_str(&batch.previous_generations).with_context(|| {
                format!(
                    "failed to parse previous_generations for batch {}",
                    batch.id
                )
            })?;
        let machine_ids: Vec<String> = serde_json::from_str(&batch.machine_ids)
            .with_context(|| format!("failed to parse machine_ids for batch {}", batch.id))?;

        for machine_id in &machine_ids {
            if let Some(prev_path) = previous_gens.get(machine_id) {
                if let Err(e) = db.set_desired_generation(machine_id, prev_path) {
                    tracing::error!(machine_id, error = %e, "failed to revert machine");
                    continue;
                }
                let machine = fleet.get_or_create(machine_id);
                machine.desired_generation = Some(DesiredGeneration {
                    hash: prev_path.clone(),
                    cache_url: None,
                    poll_hint: None,
                });
            } else {
                tracing::warn!(
                    machine_id,
                    "no previous generation recorded, skipping revert"
                );
            }
        }

        tracing::info!(
            rollout_id = %rollout.id,
            batch_id = %batch.id,
            machines = machine_ids.len(),
            "Reverted batch machines to previous generations"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_threshold_absolute() {
        assert_eq!(parse_threshold("1", 10).unwrap(), 1);
        assert_eq!(parse_threshold("5", 10).unwrap(), 5);
    }

    #[test]
    fn test_parse_threshold_percentage() {
        assert_eq!(parse_threshold("30%", 10).unwrap(), 3);
        assert_eq!(parse_threshold("10%", 20).unwrap(), 2);
        assert_eq!(parse_threshold("50%", 3).unwrap(), 2);
    }

    #[test]
    fn test_parse_threshold_100_percent() {
        assert_eq!(parse_threshold("100%", 10).unwrap(), 10);
    }

    #[test]
    fn test_parse_threshold_rejects_garbage() {
        assert!(parse_threshold("foo", 10).is_err());
        assert!(parse_threshold("foo%", 10).is_err());
        assert!(parse_threshold("-1", 10).is_err());
    }

    #[test]
    fn test_parse_threshold_rejects_out_of_range_percentage() {
        assert!(parse_threshold("150%", 10).is_err());
        assert!(parse_threshold("-5%", 10).is_err());
    }
}

#[doc(hidden)]
pub mod test_support {
    //! Synchronous entry point into a single executor tick, for integration tests.
    //!
    //! Production code uses `spawn()` which runs `tick` on a 2-second interval.
    //! Tests want deterministic advancement: one call to this function equals
    //! one tick, with the same DB queries and state mutations the real loop
    //! performs. No fake time, no mocking — just synchronous invocation.
    use super::{tick, Db, FleetState};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    pub async fn tick_for_tests(
        state: &Arc<RwLock<FleetState>>,
        db: &Arc<Db>,
    ) -> anyhow::Result<()> {
        tick(state, db).await
    }
}

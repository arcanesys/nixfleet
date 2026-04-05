use crate::db::Db;
use crate::state::FleetState;
use nixfleet_types::metrics as m;
use nixfleet_types::DesiredGeneration;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Parse a failure threshold spec into an absolute count.
///
/// - `"3"` → 3 (absolute)
/// - `"30%"` → ceil(batch_size * 0.30)
pub fn parse_threshold(spec: &str, batch_size: usize) -> usize {
    if let Some(pct_str) = spec.strip_suffix('%') {
        let pct: f64 = pct_str.parse().unwrap_or(100.0);
        (batch_size as f64 * pct / 100.0).ceil() as usize
    } else {
        spec.parse::<usize>().unwrap_or(1)
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

/// One evaluation cycle: advance all running rollouts and trigger due scheduled rollouts.
async fn tick(state: &Arc<RwLock<FleetState>>, db: &Arc<Db>) -> anyhow::Result<()> {
    // Trigger due scheduled rollouts
    if let Err(error) = trigger_due_schedules(state, db).await {
        tracing::error!(%error, "Failed to trigger scheduled rollouts");
    }

    let rollouts = db.list_rollouts_by_status(Some("running"), 100)?;

    for rollout in rollouts {
        if let Err(error) = process_rollout(state, db, &rollout).await {
            tracing::error!(rollout_id = %rollout.id, %error, "Failed to process rollout");
        }
    }

    Ok(())
}

/// Check for and trigger any scheduled rollouts whose time has come.
async fn trigger_due_schedules(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
) -> anyhow::Result<()> {
    let due = db.get_due_scheduled_rollouts()?;

    for schedule in due {
        tracing::info!(schedule_id = %schedule.id, "Triggering scheduled rollout");

        // Resolve strategy: explicit on schedule > policy > default
        let (strategy, batch_sizes, failure_threshold, on_failure, health_timeout) =
            if let Some(ref policy_id) = schedule.policy_id {
                // Load policy defaults — iterate all policies to find by ID
                let policies = db.list_policies()?;
                let policy = policies.iter().find(|p| p.id == *policy_id);
                if let Some(p) = policy {
                    (
                        schedule.strategy.clone().unwrap_or_else(|| p.strategy.clone()),
                        schedule.batch_sizes.clone().unwrap_or_else(|| p.batch_sizes.clone()),
                        schedule.failure_threshold.clone().unwrap_or_else(|| p.failure_threshold.clone()),
                        schedule.on_failure.clone().unwrap_or_else(|| p.on_failure.clone()),
                        schedule.health_timeout_secs.unwrap_or(p.health_timeout_secs),
                    )
                } else {
                    tracing::warn!(schedule_id = %schedule.id, policy_id, "Policy not found, using schedule values");
                    (
                        schedule.strategy.clone().unwrap_or_else(|| "all_at_once".to_string()),
                        schedule.batch_sizes.clone().unwrap_or_else(|| "[\"100%\"]".to_string()),
                        schedule.failure_threshold.clone().unwrap_or_else(|| "1".to_string()),
                        schedule.on_failure.clone().unwrap_or_else(|| "pause".to_string()),
                        schedule.health_timeout_secs.unwrap_or(300),
                    )
                }
            } else {
                (
                    schedule.strategy.clone().unwrap_or_else(|| "all_at_once".to_string()),
                    schedule.batch_sizes.clone().unwrap_or_else(|| "[\"100%\"]".to_string()),
                    schedule.failure_threshold.clone().unwrap_or_else(|| "1".to_string()),
                    schedule.on_failure.clone().unwrap_or_else(|| "pause".to_string()),
                    schedule.health_timeout_secs.unwrap_or(300),
                )
            };

        // Resolve target machines
        let machine_ids = if let Some(ref tags_json) = schedule.target_tags {
            let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
            db.get_machines_by_tags(&tags)?
        } else if let Some(ref hosts_json) = schedule.target_hosts {
            serde_json::from_str(hosts_json).unwrap_or_default()
        } else {
            tracing::warn!(schedule_id = %schedule.id, "No target specified");
            db.update_scheduled_rollout_status(&schedule.id, "cancelled", None)?;
            continue;
        };

        if machine_ids.is_empty() {
            tracing::warn!(schedule_id = %schedule.id, "No machines match target");
            db.update_scheduled_rollout_status(&schedule.id, "cancelled", None)?;
            continue;
        }

        // Check for active rollout conflicts
        let mut conflict = false;
        for machine_id in &machine_ids {
            if let Ok(Some(rollout_id)) = db.machine_in_active_rollout(machine_id) {
                tracing::warn!(
                    schedule_id = %schedule.id,
                    machine_id,
                    rollout_id = %rollout_id,
                    "Machine in active rollout, skipping schedule"
                );
                conflict = true;
                break;
            }
        }
        if conflict {
            continue; // Will retry on next tick
        }

        // Build batches
        let parsed_strategy = match strategy.as_str() {
            "canary" => nixfleet_types::rollout::RolloutStrategy::Canary,
            "staged" => nixfleet_types::rollout::RolloutStrategy::Staged,
            _ => nixfleet_types::rollout::RolloutStrategy::AllAtOnce,
        };
        let parsed_sizes: Option<Vec<String>> = serde_json::from_str(&batch_sizes).ok();
        let effective_sizes =
            crate::rollout::batch::effective_batch_sizes(&parsed_strategy, &parsed_sizes);
        let batches = crate::rollout::batch::build_batches(&machine_ids, &effective_sizes);

        // Capture previous generation
        let previous_generation = {
            let fleet = state.read().await;
            fleet
                .machines
                .get(&machine_ids[0])
                .and_then(|m| m.last_report.as_ref())
                .map(|r| r.current_generation.clone())
        };

        // Create the rollout
        let rollout_id = format!("r-{}", uuid::Uuid::new_v4());
        let target_tags = schedule.target_tags.as_deref();
        let target_hosts = schedule.target_hosts.as_deref();

        db.create_rollout(
            &rollout_id,
            &schedule.generation_hash,
            schedule.cache_url.as_deref(),
            &strategy,
            &serde_json::to_string(&effective_sizes).unwrap_or_default(),
            &failure_threshold,
            &on_failure,
            health_timeout,
            target_tags,
            target_hosts,
            previous_generation.as_deref(),
            &format!("schedule:{}", schedule.id),
        )?;

        // Set policy_id if applicable
        if let Some(ref policy_id) = schedule.policy_id {
            let _ = db.set_rollout_policy_id(&rollout_id, policy_id);
        }

        // Create batches
        for (i, batch_machines) in batches.iter().enumerate() {
            let batch_id = format!("{}-b{}", rollout_id, i);
            let machine_ids_json = serde_json::to_string(batch_machines).unwrap_or_default();
            db.create_rollout_batch(&batch_id, &rollout_id, i as i64, &machine_ids_json)?;
        }

        // Start the rollout
        db.update_rollout_status(&rollout_id, "running")?;

        // Emit event
        let _ = db.insert_rollout_event(
            &rollout_id,
            "status_change",
            &format!("{{\"from\":\"created\",\"to\":\"running\",\"trigger\":\"schedule:{}\"}}",
                schedule.id),
            "executor",
        );

        // Mark schedule as triggered
        db.update_scheduled_rollout_status(&schedule.id, "triggered", Some(&rollout_id))?;

        let _ = db.insert_audit_event(
            "executor",
            "schedule.triggered",
            &schedule.id,
            Some(&format!("rollout_id={rollout_id}")),
        );

        tracing::info!(
            schedule_id = %schedule.id,
            rollout_id = %rollout_id,
            machines = machine_ids.len(),
            "Scheduled rollout triggered"
        );
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
            let _ = db.insert_rollout_event(
                &rollout.id,
                "status_change",
                "{\"from\":\"running\",\"to\":\"completed\"}",
                "executor",
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

    match batch.status.as_str() {
        "pending" => deploy_batch(state, db, rollout, batch, &machine_ids).await?,
        "deploying" | "waiting_health" => {
            evaluate_batch(state, db, rollout, batch, &machine_ids, &batches).await?;
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
    let cache_url = rollout.cache_url.clone();

    // Update DB and in-memory state for each machine
    {
        let mut fleet = state.write().await;
        for machine_id in machine_ids {
            db.set_desired_generation(machine_id, &rollout.generation_hash)?;
            let machine = fleet.get_or_create(machine_id);
            machine.desired_generation = Some(DesiredGeneration {
                hash: rollout.generation_hash.clone(),
                cache_url: cache_url.clone(),
            });
        }
    }

    db.update_batch_status(&batch.id, "deploying")?;

    let _ = db.insert_rollout_event(
        &rollout.id,
        "batch_started",
        &format!(
            "{{\"batch_index\":{},\"machines\":{}}}",
            batch.batch_index,
            machine_ids.len()
        ),
        "executor",
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
async fn evaluate_batch(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
    batch: &crate::db::RolloutBatchRow,
    machine_ids: &[String],
    all_batches: &[crate::db::RolloutBatchRow],
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
        let health_reports = db.get_health_reports_since(machine_id, started_at)?;
        if !health_reports.is_empty() {
            if health_reports[0].all_passed {
                healthy_count += 1;
            } else {
                unhealthy_count += 1;
            }
        } else {
            // No health report — check regular reports for deployment failure/success
            let recent_reports = db.get_recent_reports(machine_id, 1)?;
            if let Some(report) = recent_reports.first() {
                if report.received_at.as_str() >= started_at {
                    if report.success {
                        // Deployment succeeded but no health report yet
                        pending_count += 1;
                    } else {
                        // Deployment explicitly failed
                        unhealthy_count += 1;
                    }
                } else {
                    pending_count += 1;
                }
            } else {
                pending_count += 1;
            }
        }
    }

    // If any machines haven't reported yet, check health timeout
    if pending_count > 0 {
        let timed_out = if let Ok(batch_start) =
            chrono::NaiveDateTime::parse_from_str(started_at, "%Y-%m-%d %H:%M:%S")
        {
            let batch_start_utc = chrono::TimeZone::from_utc_datetime(&chrono::Utc, &batch_start);
            let elapsed = chrono::Utc::now()
                .signed_duration_since(batch_start_utc)
                .num_seconds();
            elapsed >= rollout.health_timeout
        } else {
            false
        };

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

    // All machines have reported — evaluate
    let threshold = parse_threshold(&rollout.failure_threshold, machine_ids.len());

    if unhealthy_count < threshold {
        // Batch succeeded
        db.update_batch_status(&batch.id, "succeeded")?;
        let _ = db.insert_rollout_event(
            &rollout.id,
            "batch_completed",
            &format!(
                "{{\"batch_index\":{},\"healthy\":{healthy_count},\"unhealthy\":{unhealthy_count}}}",
                batch.batch_index
            ),
            "executor",
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
        let _ = db.insert_rollout_event(
            &rollout.id,
            "batch_failed",
            &format!(
                "{{\"batch_index\":{},\"unhealthy\":{unhealthy_count},\"threshold\":{threshold}}}",
                batch.batch_index
            ),
            "executor",
        );
        db.insert_audit_event(
            "executor",
            "batch.failed",
            &batch.id,
            Some(&format!(
                "Batch {} failed: {unhealthy_count} unhealthy >= threshold {threshold}",
                batch.batch_index
            )),
        )?;

        match rollout.on_failure.as_str() {
            "pause" => {
                db.update_rollout_status(&rollout.id, "paused")?;
                let _ = db.insert_rollout_event(
                    &rollout.id,
                    "status_change",
                    "{\"from\":\"running\",\"to\":\"paused\",\"reason\":\"batch failure\"}",
                    "executor",
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
                let _ = db.insert_rollout_event(
                    &rollout.id,
                    "status_change",
                    "{\"from\":\"running\",\"to\":\"failed\",\"reason\":\"batch failure + revert\"}",
                    "executor",
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

/// Revert all machines in completed (succeeded) batches to the previous generation.
async fn revert_completed_batches(
    state: &Arc<RwLock<FleetState>>,
    db: &Arc<Db>,
    rollout: &crate::db::RolloutRow,
    all_batches: &[crate::db::RolloutBatchRow],
) -> anyhow::Result<()> {
    let previous_gen = match &rollout.previous_generation {
        Some(gen) if !gen.is_empty() => gen.clone(),
        _ => {
            tracing::warn!(
                rollout_id = %rollout.id,
                "No previous generation to revert to"
            );
            return Ok(());
        }
    };

    let mut fleet = state.write().await;

    for batch in all_batches {
        if batch.status != "succeeded" {
            continue;
        }

        let machine_ids: Vec<String> = serde_json::from_str(&batch.machine_ids)?;
        for machine_id in &machine_ids {
            db.set_desired_generation(machine_id, &previous_gen)?;
            let machine = fleet.get_or_create(machine_id);
            machine.desired_generation = Some(DesiredGeneration {
                hash: previous_gen.clone(),
                cache_url: None,
            });
        }

        tracing::info!(
            rollout_id = %rollout.id,
            batch_id = %batch.id,
            machines = machine_ids.len(),
            previous_generation = %previous_gen,
            "Reverted batch machines to previous generation"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_threshold_absolute() {
        assert_eq!(parse_threshold("1", 10), 1);
        assert_eq!(parse_threshold("5", 10), 5);
    }

    #[test]
    fn test_parse_threshold_percentage() {
        assert_eq!(parse_threshold("30%", 10), 3);
        assert_eq!(parse_threshold("10%", 20), 2);
        assert_eq!(parse_threshold("50%", 3), 2);
    }

    #[test]
    fn test_parse_threshold_100_percent() {
        assert_eq!(parse_threshold("100%", 10), 10);
    }
}

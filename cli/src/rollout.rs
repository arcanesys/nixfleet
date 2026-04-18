use crate::display;
use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{RolloutDetail, RolloutStatus};
use std::time::{Duration, Instant};

/// Default upper bound on how long `deploy --wait` / `rollout status --wait`
/// will block before aborting with a timeout error. Keeps CI jobs from
/// hanging forever on a stuck rollout.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Default poll cadence inside the wait loop. Low enough to feel
/// interactive, high enough not to hammer the control plane.
const WAIT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// GET /api/v1/rollouts — list rollouts, optionally filtered by status.
pub async fn list(
    client: &reqwest::Client,
    cp_url: &str,
    status_filter: Option<&str>,
    json: bool,
) -> Result<()> {
    let mut url = format!("{}/api/v1/rollouts", cp_url);
    if let Some(status) = status_filter {
        url.push_str(&format!("?status={}", status));
    }

    let resp = client
        .get(&url)
        .send()
        .await
        .context("failed to reach control plane")?;

    let resp = crate::client::check_response(resp).await?;

    let rollouts: Vec<RolloutDetail> = resp.json().await.context("failed to parse rollout list")?;

    if rollouts.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No rollouts found.");
        }
        return Ok(());
    }

    let rows: Vec<Vec<String>> = rollouts
        .iter()
        .map(|r| {
            vec![
                r.id.clone(),
                display::color_status(&r.status.to_string()),
                r.strategy.to_string(),
                r.batches.len().to_string(),
                r.created_at.format("%Y-%m-%d %H:%M").to_string(),
                truncate(&r.release_id, 30),
            ]
        })
        .collect();

    display::print_list(
        json,
        &["ID", "STATUS", "STRATEGY", "BATCHES", "CREATED", "RELEASE"],
        &rows,
        &rollouts,
    );

    if !json {
        println!("\n{} rollout(s)", rollouts.len());
    }

    Ok(())
}

/// GET /api/v1/rollouts/{id} — show rollout detail with batch breakdown.
pub async fn status(client: &reqwest::Client, cp_url: &str, id: &str, json: bool) -> Result<()> {
    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("failed to reach control plane")?;

    let resp = crate::client::check_response(resp).await?;

    let rollout: RolloutDetail = resp
        .json()
        .await
        .context("failed to parse rollout detail")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&rollout)?);
        return Ok(());
    }

    print_rollout_detail(&rollout);
    Ok(())
}

/// POST /api/v1/rollouts/{id}/resume — resume a paused rollout.
pub async fn resume(client: &reqwest::Client, cp_url: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/v1/rollouts/{}/resume", cp_url, id);

    let resp = client
        .post(&url)
        .send()
        .await
        .context("failed to reach control plane")?;

    crate::client::check_response(resp).await?;

    println!("Rollout {} resumed.", id);
    Ok(())
}

/// POST /api/v1/rollouts/{id}/cancel — cancel a rollout.
pub async fn cancel(client: &reqwest::Client, cp_url: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/v1/rollouts/{}/cancel", cp_url, id);

    let resp = client
        .post(&url)
        .send()
        .await
        .context("failed to reach control plane")?;

    crate::client::check_response(resp).await?;

    println!("Rollout {} cancelled.", id);
    Ok(())
}

/// DELETE /api/v1/rollouts/{id} — delete a terminal rollout.
pub async fn delete(client: &reqwest::Client, cp_url: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);

    let resp = client
        .delete(&url)
        .send()
        .await
        .context("failed to DELETE rollout")?;

    let status = resp.status();
    if status.as_u16() == 204 || status.is_success() {
        println!("Rollout {} deleted.", id);
        return Ok(());
    }
    if status.as_u16() == 409 {
        let body = crate::client::read_error_body(resp).await;
        anyhow::bail!("Rollout {} cannot be deleted: {}", id, body);
    }
    if status.as_u16() == 404 {
        anyhow::bail!("Rollout {} not found", id);
    }
    let body = crate::client::read_error_body(resp).await;
    anyhow::bail!("failed to delete rollout: {} {}", status, body);
}

/// Poll a rollout until it reaches a terminal state, printing progress every interval.
///
/// Falls back to [`DEFAULT_WAIT_TIMEOUT`] if `max_wait` is `None`.
/// Using `Some(Duration::ZERO)` disables the timeout (wait forever).
pub async fn wait_for_completion(
    client: &reqwest::Client,
    cp_url: &str,
    id: &str,
    max_wait: Option<Duration>,
) -> Result<()> {
    use indicatif::{ProgressBar, ProgressStyle};

    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);
    let timeout = max_wait.unwrap_or(DEFAULT_WAIT_TIMEOUT);
    let started = Instant::now();

    let is_tty = console::Term::stderr().is_term();
    let pb = if is_tty {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap(),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(120));
        Some(pb)
    } else {
        None
    };

    loop {
        let resp = client
            .get(&url)
            .send()
            .await
            .context("failed to reach control plane")?;

        if !resp.status().is_success() {
            if let Some(ref pb) = pb {
                pb.finish_and_clear();
            }
            bail!(
                "Control plane returned {}: {}",
                resp.status(),
                resp.text()
                    .await
                    .unwrap_or_else(|e| format!("<failed to read body: {e}>"))
            );
        }

        let rollout: RolloutDetail = resp
            .json()
            .await
            .context("failed to parse rollout detail")?;

        let total_machines: usize = rollout.batches.iter().map(|b| b.machine_ids.len()).sum();
        let healthy_machines: usize = rollout
            .batches
            .iter()
            .flat_map(|b| b.machine_health.values())
            .filter(|h| matches!(h, nixfleet_types::rollout::MachineHealthStatus::Healthy))
            .count();
        let current_batch = rollout
            .batches
            .iter()
            .position(|b| {
                matches!(
                    b.status,
                    nixfleet_types::rollout::BatchStatus::Deploying
                        | nixfleet_types::rollout::BatchStatus::WaitingHealth
                )
            })
            .map(|i| i + 1)
            .unwrap_or(0);

        let msg = format!(
            "Rollout {} \u{2014} batch {}/{} \u{2014} {}/{} healthy \u{2014} {}",
            id,
            current_batch,
            rollout.batches.len(),
            healthy_machines,
            total_machines,
            rollout.status,
        );

        if let Some(ref pb) = pb {
            pb.set_message(msg);
        } else {
            tracing::info!(
                batch = current_batch,
                total_batches = rollout.batches.len(),
                healthy = healthy_machines,
                total = total_machines,
                status = %rollout.status,
                "rollout progress"
            );
        }

        if !rollout.status.is_active() {
            if let Some(ref pb) = pb {
                pb.finish_and_clear();
            }
            println!(
                "Rollout {} finished: {} ({} machines, {} batches)",
                id,
                rollout.status,
                total_machines,
                rollout.batches.len()
            );
            if rollout.status == RolloutStatus::Failed {
                bail!("Rollout failed");
            }
            return Ok(());
        }

        if !timeout.is_zero() && started.elapsed() >= timeout {
            if let Some(ref pb) = pb {
                pb.finish_and_clear();
            }
            bail!(
                "Timed out after {}s waiting for rollout {} to finish (last status: {}). \
                 Re-run with --wait-timeout 0 to block indefinitely, or inspect with \
                 `nixfleet rollout status {}`.",
                timeout.as_secs(),
                id,
                rollout.status,
                id,
            );
        }

        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
}

fn print_rollout_detail(rollout: &RolloutDetail) {
    display::print_detail(&[
        ("Rollout", rollout.id.clone()),
        ("Status", display::color_status(&rollout.status.to_string())),
        ("Strategy", rollout.strategy.to_string()),
        ("Release", rollout.release_id.clone()),
        ("On failure", rollout.on_failure.to_string()),
        ("Fail threshold", rollout.failure_threshold.clone()),
        ("Health timeout", format!("{}s", rollout.health_timeout)),
        ("Created by", rollout.created_by.clone()),
        (
            "Created at",
            rollout
                .created_at
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
        ),
        (
            "Updated at",
            rollout
                .updated_at
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
        ),
    ]);

    println!();
    let batch_rows: Vec<Vec<String>> = rollout
        .batches
        .iter()
        .map(|batch| {
            let healthy = batch
                .machine_health
                .values()
                .filter(|h| matches!(h, nixfleet_types::rollout::MachineHealthStatus::Healthy))
                .count();
            let total = batch.machine_ids.len();
            vec![
                batch.batch_index.to_string(),
                display::color_status(&batch.status.to_string()),
                total.to_string(),
                format!("{}/{}", healthy, total),
            ]
        })
        .collect();

    display::print_table(&["BATCH", "STATUS", "MACHINES", "HEALTHY"], &batch_rows);

    for batch in &rollout.batches {
        for machine_id in &batch.machine_ids {
            let health = batch
                .machine_health
                .get(machine_id)
                .map(|h| h.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("  {} → {}", machine_id, display::color_status(&health));
        }
    }

    if !rollout.events.is_empty() {
        println!("\nTimeline:");
        for event in &rollout.events {
            println!(
                "  {}  {:<20} ({})",
                event.created_at.format("%Y-%m-%dT%H:%M:%SZ"),
                event.event_type,
                event.actor,
            );
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{RolloutDetail, RolloutStatus};
use std::time::{Duration, Instant};

/// Default upper bound on how long `deploy --wait` / `rollout status --wait`
/// will block before aborting with a timeout error. Keeps CI jobs from
/// hanging forever on a stuck rollout.
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Default poll cadence inside the wait loop. Low enough to feel
/// interactive, high enough not to hammer the control plane.
const WAIT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// GET /api/v1/rollouts — list rollouts, optionally filtered by status.
pub async fn list(
    client: &reqwest::Client,
    cp_url: &str,
    status_filter: Option<&str>,
) -> Result<()> {
    let mut url = format!("{}/api/v1/rollouts", cp_url);
    if let Some(status) = status_filter {
        url.push_str(&format!("?status={}", status));
    }

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            crate::client::read_error_body(resp).await
        );
    }

    let rollouts: Vec<RolloutDetail> = resp.json().await.context("Failed to parse rollout list")?;

    if rollouts.is_empty() {
        println!("No rollouts found.");
        return Ok(());
    }

    println!(
        "{:<38} {:<12} {:<14} {:<8} {:<20} RELEASE",
        "ID", "STATUS", "STRATEGY", "BATCHES", "CREATED"
    );
    println!("{}", "-".repeat(110));

    for rollout in &rollouts {
        let created = rollout.created_at.format("%Y-%m-%d %H:%M:%S");
        let release = truncate(&rollout.release_id, 30);
        println!(
            "{:<38} {:<12} {:<14} {:<8} {:<20} {}",
            rollout.id,
            rollout.status,
            rollout.strategy,
            rollout.batches.len(),
            created,
            release,
        );
    }

    println!("\n{} rollout(s)", rollouts.len());
    Ok(())
}

/// GET /api/v1/rollouts/{id} — show rollout detail with batch breakdown.
pub async fn status(client: &reqwest::Client, cp_url: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            crate::client::read_error_body(resp).await
        );
    }

    let rollout: RolloutDetail = resp
        .json()
        .await
        .context("Failed to parse rollout detail")?;
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
        .context("Failed to reach control plane")?;

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            crate::client::read_error_body(resp).await
        );
    }

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
        .context("Failed to reach control plane")?;

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            crate::client::read_error_body(resp).await
        );
    }

    println!("Rollout {} cancelled.", id);
    Ok(())
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
    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);
    let timeout = max_wait.unwrap_or(DEFAULT_WAIT_TIMEOUT);
    let started = Instant::now();

    loop {
        let resp = client
            .get(&url)
            .send()
            .await
            .context("Failed to reach control plane")?;

        if !resp.status().is_success() {
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
            .context("Failed to parse rollout detail")?;

        print_progress(&rollout);

        if !rollout.status.is_active() {
            println!("\nRollout {} finished with status: {}", id, rollout.status);
            if rollout.status == RolloutStatus::Failed {
                bail!("Rollout failed");
            }
            return Ok(());
        }

        if !timeout.is_zero() && started.elapsed() >= timeout {
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
    println!("Rollout:       {}", rollout.id);
    println!("Status:        {}", rollout.status);
    println!("Strategy:      {}", rollout.strategy);
    println!("Release:       {}", rollout.release_id);
    println!("On failure:    {}", rollout.on_failure);
    println!("Fail threshold:{}", rollout.failure_threshold);
    println!("Health timeout:{}s", rollout.health_timeout);
    println!("Created by:    {}", rollout.created_by);
    println!(
        "Created at:    {}",
        rollout.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!(
        "Updated at:    {}",
        rollout.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    println!("\nBatches ({}):", rollout.batches.len());
    println!("  {:<6} {:<16} {:<8} HEALTH", "INDEX", "STATUS", "MACHINES");
    println!("  {}", "-".repeat(60));

    for batch in &rollout.batches {
        let healthy = batch
            .machine_health
            .values()
            .filter(|h| matches!(h, nixfleet_types::rollout::MachineHealthStatus::Healthy))
            .count();
        let total = batch.machine_ids.len();

        println!(
            "  {:<6} {:<16} {:<8} {}/{}",
            batch.batch_index, batch.status, total, healthy, total,
        );

        for machine_id in &batch.machine_ids {
            let health = batch
                .machine_health
                .get(machine_id)
                .map(|h| h.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("    {} -> {}", machine_id, health);
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

fn print_progress(rollout: &RolloutDetail) {
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

    println!(
        "[{}] batch {}/{} | {}/{} machines healthy | status: {}",
        rollout.id,
        current_batch,
        rollout.batches.len(),
        healthy_machines,
        total_machines,
        rollout.status,
    );
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

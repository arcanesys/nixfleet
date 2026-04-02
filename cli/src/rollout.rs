use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{RolloutDetail, RolloutStatus};

use crate::client::api_client;

/// GET /api/v1/rollouts — list rollouts, optionally filtered by status.
pub async fn list(cp_url: &str, api_key: &str, status_filter: Option<&str>) -> Result<()> {
    let client = api_client(api_key);
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
            resp.text().await.unwrap_or_default()
        );
    }

    let rollouts: Vec<RolloutDetail> = resp.json().await.context("Failed to parse rollout list")?;

    if rollouts.is_empty() {
        println!("No rollouts found.");
        return Ok(());
    }

    println!(
        "{:<38} {:<12} {:<14} {:<8} {:<20} GENERATION",
        "ID", "STATUS", "STRATEGY", "BATCHES", "CREATED"
    );
    println!("{}", "-".repeat(110));

    for rollout in &rollouts {
        let created = rollout.created_at.format("%Y-%m-%d %H:%M:%S");
        let generation = truncate(&rollout.generation_hash, 30);
        println!(
            "{:<38} {:<12} {:<14} {:<8} {:<20} {}",
            rollout.id,
            rollout.status,
            rollout.strategy,
            rollout.batches.len(),
            created,
            generation,
        );
    }

    println!("\n{} rollout(s)", rollouts.len());
    Ok(())
}

/// GET /api/v1/rollouts/{id} — show rollout detail with batch breakdown.
pub async fn status(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = api_client(api_key);
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
            resp.text().await.unwrap_or_default()
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
pub async fn resume(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = api_client(api_key);
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
            resp.text().await.unwrap_or_default()
        );
    }

    println!("Rollout {} resumed.", id);
    Ok(())
}

/// POST /api/v1/rollouts/{id}/cancel — cancel a rollout.
pub async fn cancel(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = api_client(api_key);
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
            resp.text().await.unwrap_or_default()
        );
    }

    println!("Rollout {} cancelled.", id);
    Ok(())
}

/// Poll a rollout until it reaches a terminal state, printing progress every interval.
pub async fn wait_for_completion(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = api_client(api_key);
    let url = format!("{}/api/v1/rollouts/{}", cp_url, id);

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
                resp.text().await.unwrap_or_default()
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

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

fn print_rollout_detail(rollout: &RolloutDetail) {
    println!("Rollout:       {}", rollout.id);
    println!("Status:        {}", rollout.status);
    println!("Strategy:      {}", rollout.strategy);
    println!("Generation:    {}", rollout.generation_hash);
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

use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::ScheduledRollout;

/// GET /api/v1/schedules — list scheduled rollouts.
pub async fn list(
    client: &reqwest::Client,
    cp_url: &str,
    status_filter: Option<&str>,
) -> Result<()> {
    let mut url = format!("{}/api/v1/schedules", cp_url);
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

    let schedules: Vec<ScheduledRollout> =
        resp.json().await.context("Failed to parse schedule list")?;

    if schedules.is_empty() {
        println!("No scheduled rollouts found.");
        return Ok(());
    }

    println!(
        "{:<42} {:<12} {:<22} {:<30}",
        "ID", "STATUS", "SCHEDULED AT", "RELEASE"
    );
    println!("{}", "-".repeat(106));

    for sched in &schedules {
        let scheduled_at = sched.scheduled_at.format("%Y-%m-%d %H:%M:%S UTC");
        let release = truncate(&sched.release_id, 28);
        println!(
            "{:<42} {:<12} {:<22} {}",
            sched.id, sched.status, scheduled_at, release,
        );
    }

    println!("\n{} schedule(s)", schedules.len());
    Ok(())
}

/// POST /api/v1/schedules/{id}/cancel — cancel a scheduled rollout.
pub async fn cancel(client: &reqwest::Client, cp_url: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/v1/schedules/{}/cancel", cp_url, id);

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

    println!("Scheduled rollout {} cancelled.", id);
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

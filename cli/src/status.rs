use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

pub async fn run(cp_url: &str, json_output: bool) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/machines", cp_url);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Control plane returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    let machines: Vec<MachineStatus> = resp.json().await.context("Failed to parse machine list")?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&machines)?);
        return Ok(());
    }

    if machines.is_empty() {
        println!("No machines registered with the control plane.");
        return Ok(());
    }

    // Table output
    println!(
        "{:<20} {:<10} {:<15} {:<45} {:<45} LAST SEEN",
        "HOST", "STATE", "LIFECYCLE", "CURRENT", "DESIRED"
    );
    println!("{}", "-".repeat(155));

    for m in &machines {
        let current = truncate_store_path(&m.current_generation, 43);
        let desired = m
            .desired_generation
            .as_deref()
            .map(|d| truncate_store_path(d, 43))
            .unwrap_or_else(|| "(none)".to_string());
        let last_seen = m
            .last_report
            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "never".to_string());

        let state_indicator = match m.system_state.as_str() {
            "ok" => "ok",
            "error" => "ERROR",
            _ => "?",
        };

        println!(
            "{:<20} {:<10} {:<15} {:<45} {:<45} {}",
            m.machine_id,
            state_indicator,
            m.lifecycle.to_string(),
            current,
            desired,
            last_seen
        );
    }

    let in_sync = machines
        .iter()
        .filter(|m| {
            m.desired_generation
                .as_deref()
                .map(|d| d == m.current_generation)
                .unwrap_or(false)
        })
        .count();

    println!("\n{}/{} hosts in sync", in_sync, machines.len());

    Ok(())
}

/// Truncate a store path for display, keeping the hash prefix and derivation name.
fn truncate_store_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len || path.is_empty() {
        return path.to_string();
    }
    format!("{}...", &path[..max_len - 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_store_path_short() {
        assert_eq!(truncate_store_path("/nix/store/abc", 50), "/nix/store/abc");
    }

    #[test]
    fn test_truncate_store_path_long() {
        let long_path = "/nix/store/abc123def456ghi789jkl012mno345pqr678-nixos-system-web-01-25.05";
        let truncated = truncate_store_path(long_path, 40);
        assert_eq!(truncated.len(), 40);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_truncate_store_path_empty() {
        assert_eq!(truncate_store_path("", 50), "");
    }
}

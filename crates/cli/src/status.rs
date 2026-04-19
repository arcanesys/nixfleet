use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

use crate::display;

/// Check if a machine's last report exceeds the stale threshold.
/// Returns false if the machine has never reported (seconds_since_last_report is None).
fn is_stale(m: &MachineStatus, threshold: u64) -> bool {
    m.seconds_since_last_report
        .map(|s| s > threshold)
        .unwrap_or(false)
}

pub async fn run(
    client: &reqwest::Client,
    cp_url: &str,
    json: bool,
    stale_threshold: u64,
) -> Result<()> {
    let url = format!("{}/api/v1/machines", cp_url);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("failed to reach control plane")?;

    let resp = crate::client::check_response(resp).await?;

    let machines: Vec<MachineStatus> = resp.json().await.context("failed to parse machine list")?;

    if machines.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No machines registered with the control plane.");
        }
        return Ok(());
    }

    let rows: Vec<Vec<String>> = machines
        .iter()
        .map(|m| {
            let state_str = match m.system_state.as_str() {
                "ok" => "ok",
                "error" => "ERROR",
                _ => "?",
            };
            let is_stale = is_stale(m, stale_threshold);
            let state = if is_stale {
                format!("{state_str} (stale)")
            } else {
                state_str.to_string()
            };
            let sync = match &m.desired_generation {
                Some(desired) if desired == &m.current_generation => "in_sync".to_string(),
                Some(_) => "outdated".to_string(),
                None => "\u{2014}".to_string(),
            };
            let current = if m.current_generation.is_empty() {
                "\u{2014}".to_string()
            } else {
                display::format_store_path_compact(&m.current_generation, 50)
            };
            let desired = m
                .desired_generation
                .as_deref()
                .map(|d| display::format_store_path_compact(d, 50))
                .unwrap_or_else(|| "(none)".to_string());
            let last_seen = m
                .last_report
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            vec![
                m.machine_id.clone(),
                display::color_status(&state),
                display::color_status(&m.lifecycle.to_string()),
                display::color_status(&sync),
                current,
                desired,
                last_seen,
            ]
        })
        .collect();

    display::print_list(
        json,
        &[
            "HOST",
            "STATE",
            "LIFECYCLE",
            "SYNC",
            "CURRENT",
            "DESIRED",
            "LAST SEEN",
        ],
        &rows,
        &machines,
    );

    if !json {
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
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_types::{MachineLifecycle, MachineStatus};

    fn make_machine(system_state: &str, seconds_since: Option<u64>) -> MachineStatus {
        MachineStatus {
            machine_id: "web-01".to_string(),
            current_generation: "/nix/store/abc".to_string(),
            desired_generation: None,
            agent_version: "0.1.0".to_string(),
            system_state: system_state.to_string(),
            uptime_seconds: 3600,
            last_report: Some(chrono::Utc::now()),
            lifecycle: MachineLifecycle::Active,
            tags: vec![],
            seconds_since_last_report: seconds_since,
        }
    }

    #[test]
    fn test_stale_annotation() {
        let threshold: u64 = 600;

        // Fresh machine — no stale annotation
        assert!(!is_stale(&make_machine("ok", Some(300)), threshold));

        // Stale machine
        assert!(is_stale(&make_machine("ok", Some(900)), threshold));

        // Never reported — not stale (already shows "?" / "never")
        assert!(!is_stale(&make_machine("unknown", None), threshold));

        // Error + stale
        assert!(is_stale(&make_machine("error", Some(1200)), threshold));

        // Exactly at threshold — not stale (> not >=)
        assert!(!is_stale(&make_machine("ok", Some(600)), threshold));
    }
}

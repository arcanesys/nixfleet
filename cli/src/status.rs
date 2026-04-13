use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

use crate::display;

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
            let is_stale = m
                .seconds_since_last_report
                .map(|s| s > stale_threshold)
                .unwrap_or(false);
            let state = if is_stale {
                format!("{state_str} (stale)")
            } else {
                state_str.to_string()
            };
            let current = display::truncate_store_path(&m.current_generation, 36);
            let desired = m
                .desired_generation
                .as_deref()
                .map(|d| display::truncate_store_path(d, 36))
                .unwrap_or_else(|| "(none)".to_string());
            let last_seen = m
                .last_report
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            vec![
                m.machine_id.clone(),
                display::color_status(&state),
                display::color_status(&m.lifecycle.to_string()),
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
        let m = make_machine("ok", Some(300));
        let is_stale = m
            .seconds_since_last_report
            .map(|s| s > threshold)
            .unwrap_or(false);
        assert!(!is_stale);

        // Stale machine
        let m = make_machine("ok", Some(900));
        let is_stale = m
            .seconds_since_last_report
            .map(|s| s > threshold)
            .unwrap_or(false);
        assert!(is_stale);

        // Never reported — not stale (already shows "?" / "never")
        let m = make_machine("unknown", None);
        let is_stale = m
            .seconds_since_last_report
            .map(|s| s > threshold)
            .unwrap_or(false);
        assert!(!is_stale);

        // Error + stale
        let m = make_machine("error", Some(1200));
        let is_stale = m
            .seconds_since_last_report
            .map(|s| s > threshold)
            .unwrap_or(false);
        assert!(is_stale);
    }
}

use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

use crate::display;

fn is_stale(m: &MachineStatus, threshold_secs: i64) -> bool {
    match m.last_report {
        Some(last) => {
            let age = chrono::Utc::now().signed_duration_since(last);
            age.num_seconds() > threshold_secs
        }
        None => false,
    }
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

    let stale_threshold = stale_threshold as i64;

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
            let current = display::format_store_path_compact(&m.current_generation, 40);
            let desired = m
                .desired_generation
                .as_deref()
                .map(|d| display::format_store_path_compact(d, 40))
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

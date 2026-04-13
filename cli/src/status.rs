use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

use crate::display;

pub async fn run(client: &reqwest::Client, cp_url: &str, json: bool) -> Result<()> {
    let url = format!("{}/api/v1/machines", cp_url);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    let resp = crate::client::check_response(resp).await?;

    let machines: Vec<MachineStatus> = resp.json().await.context("Failed to parse machine list")?;

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
            let state = match m.system_state.as_str() {
                "ok" => "ok",
                "error" => "ERROR",
                _ => "?",
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
                display::color_status(state),
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

use anyhow::{Context, Result};
use nixfleet_types::MachineStatus;

use crate::display;

/// GET /api/v1/machines — list machines, optionally filtered by tags.
pub async fn list(
    client: &reqwest::Client,
    cp_url: &str,
    tag_filters: &[String],
    json: bool,
) -> Result<()> {
    let url = format!("{}/api/v1/machines", cp_url);

    let resp = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    let resp = crate::client::check_response(resp).await?;

    let machines: Vec<MachineStatus> = resp.json().await.context("Failed to parse machine list")?;

    let filtered: Vec<&MachineStatus> = if tag_filters.is_empty() {
        machines.iter().collect()
    } else {
        machines
            .iter()
            .filter(|m| tag_filters.iter().any(|f| m.tags.iter().any(|t| t == f)))
            .collect()
    };

    if filtered.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No machines found.");
        }
        return Ok(());
    }

    let rows: Vec<Vec<String>> = filtered
        .iter()
        .map(|m| {
            let tags = if m.tags.is_empty() {
                "(none)".to_string()
            } else {
                m.tags.join(", ")
            };
            vec![
                m.machine_id.clone(),
                display::color_status(&m.lifecycle.to_string()),
                display::color_status(&m.system_state),
                tags,
            ]
        })
        .collect();

    display::print_list(json, &["ID", "LIFECYCLE", "STATE", "TAGS"], &rows, &filtered);

    if !json {
        println!("\n{} machine(s)", filtered.len());
    }
    Ok(())
}

/// DELETE /api/v1/machines/{id}/tags/{tag} — remove a tag from a machine.
pub async fn untag(
    client: &reqwest::Client,
    cp_url: &str,
    machine_id: &str,
    tag: &str,
) -> Result<()> {
    let url = format!("{}/api/v1/machines/{}/tags/{}", cp_url, machine_id, tag);

    let resp = client
        .delete(&url)
        .send()
        .await
        .context("Failed to reach control plane")?;

    crate::client::check_response(resp).await?;

    println!("Tag '{}' removed from {}.", tag, machine_id);
    Ok(())
}

/// POST /api/v1/machines/{id}/register — register a machine with the control plane.
pub async fn register(
    client: &reqwest::Client,
    cp_url: &str,
    machine_id: &str,
    tags: &[String],
) -> Result<()> {
    let url = format!("{}/api/v1/machines/{}/register", cp_url, machine_id);
    let body = serde_json::json!({ "tags": tags });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("failed to reach control plane")?;

    crate::client::check_response(resp).await?;

    if tags.is_empty() {
        println!("Machine '{}' registered.", machine_id);
    } else {
        println!(
            "Machine '{}' registered with tags: {}",
            machine_id,
            tags.join(", ")
        );
    }
    Ok(())
}

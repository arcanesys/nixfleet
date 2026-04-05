use anyhow::{bail, Context, Result};
use nixfleet_types::MachineStatus;

/// GET /api/v1/machines — list machines, optionally filtered by tag.
pub async fn list(
    client: &reqwest::Client,
    cp_url: &str,
    tag_filter: Option<&str>,
) -> Result<()> {
    let url = format!("{}/api/v1/machines", cp_url);

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

    let machines: Vec<MachineStatus> = resp.json().await.context("Failed to parse machine list")?;

    let filtered: Vec<&MachineStatus> = if let Some(tag) = tag_filter {
        machines
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == tag))
            .collect()
    } else {
        machines.iter().collect()
    };

    if filtered.is_empty() {
        println!("No machines found.");
        return Ok(());
    }

    println!("{:<20} {:<12} {:<12} TAGS", "ID", "LIFECYCLE", "STATE");
    println!("{}", "-".repeat(70));

    for machine in &filtered {
        let tags = if machine.tags.is_empty() {
            "(none)".to_string()
        } else {
            machine.tags.join(", ")
        };
        println!(
            "{:<20} {:<12} {:<12} {}",
            machine.machine_id, machine.lifecycle, machine.system_state, tags,
        );
    }

    println!("\n{} machine(s)", filtered.len());
    Ok(())
}

/// PUT /api/v1/machines/{id}/tags — set tags on a machine.
pub async fn tag(
    client: &reqwest::Client,
    cp_url: &str,
    machine_id: &str,
    tags: &[String],
) -> Result<()> {
    let url = format!("{}/api/v1/machines/{}/tags", cp_url, machine_id);

    let resp = client
        .post(&url)
        .json(tags)
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

    println!("Tags set on {}: {}", machine_id, tags.join(", "));
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

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

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

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

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

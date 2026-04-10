use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{PolicyRequest, RolloutPolicy};

/// POST /api/v1/policies — create a new policy.
pub async fn create(client: &reqwest::Client, cp_url: &str, request: &PolicyRequest) -> Result<()> {
    let url = format!("{}/api/v1/policies", cp_url);

    let resp = client
        .post(&url)
        .json(request)
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

    let policy: RolloutPolicy = resp
        .json()
        .await
        .context("Failed to parse policy response")?;
    println!("Policy '{}' created (id: {})", policy.name, policy.id);
    println!("  Strategy:         {}", policy.strategy);
    println!("  Batch sizes:      {}", policy.batch_sizes.join(", "));
    println!("  Failure threshold:{}", policy.failure_threshold);
    println!("  On failure:       {}", policy.on_failure);
    println!("  Health timeout:   {}s", policy.health_timeout_secs);
    Ok(())
}

/// GET /api/v1/policies — list all policies.
pub async fn list(client: &reqwest::Client, cp_url: &str) -> Result<()> {
    let url = format!("{}/api/v1/policies", cp_url);

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

    let policies: Vec<RolloutPolicy> = resp.json().await.context("Failed to parse policy list")?;

    if policies.is_empty() {
        println!("No policies found.");
        return Ok(());
    }

    println!(
        "{:<24} {:<14} {:<18} {:<10} {:<8} TIMEOUT",
        "NAME", "STRATEGY", "BATCH SIZES", "THRESHOLD", "ON_FAIL"
    );
    println!("{}", "-".repeat(90));

    for policy in &policies {
        println!(
            "{:<24} {:<14} {:<18} {:<10} {:<8} {}s",
            policy.name,
            policy.strategy,
            policy.batch_sizes.join(","),
            policy.failure_threshold,
            policy.on_failure,
            policy.health_timeout_secs,
        );
    }

    println!("\n{} policy(ies)", policies.len());
    Ok(())
}

/// GET /api/v1/policies/{name} — show a policy.
pub async fn get(client: &reqwest::Client, cp_url: &str, name: &str) -> Result<()> {
    let url = format!("{}/api/v1/policies/{}", cp_url, name);

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

    let policy: RolloutPolicy = resp.json().await.context("Failed to parse policy")?;
    println!("Policy:          {}", policy.name);
    println!("ID:              {}", policy.id);
    println!("Strategy:        {}", policy.strategy);
    println!("Batch sizes:     {}", policy.batch_sizes.join(", "));
    println!("Fail threshold:  {}", policy.failure_threshold);
    println!("On failure:      {}", policy.on_failure);
    println!("Health timeout:  {}s", policy.health_timeout_secs);
    println!(
        "Created at:      {}",
        policy.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!(
        "Updated at:      {}",
        policy.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

/// PUT /api/v1/policies/{name} — update a policy.
pub async fn update(
    client: &reqwest::Client,
    cp_url: &str,
    name: &str,
    request: &PolicyRequest,
) -> Result<()> {
    let url = format!("{}/api/v1/policies/{}", cp_url, name);

    let resp = client
        .put(&url)
        .json(request)
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

    println!("Policy '{}' updated.", name);
    Ok(())
}

/// DELETE /api/v1/policies/{name} — delete a policy.
pub async fn delete(client: &reqwest::Client, cp_url: &str, name: &str) -> Result<()> {
    let url = format!("{}/api/v1/policies/{}", cp_url, name);

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

    println!("Policy '{}' deleted.", name);
    Ok(())
}

use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{
    CreateRolloutRequest, CreateRolloutResponse, OnFailure, RolloutStrategy, RolloutTarget,
};
use std::collections::HashMap;
use std::process::Stdio;

/// Discover NixOS host names from the flake by evaluating nixosConfigurations attribute names.
async fn discover_hosts(flake: &str) -> Result<Vec<String>> {
    let output = tokio::process::Command::new("nix")
        .args([
            "eval",
            &format!("{}#nixosConfigurations", flake),
            "--apply",
            "builtins.attrNames",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to run nix eval for host discovery")?;

    if !output.status.success() {
        bail!(
            "nix eval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let hosts: Vec<String> =
        serde_json::from_slice(&output.stdout).context("Failed to parse host list")?;
    Ok(hosts)
}

/// Filter hosts by a glob-like pattern. Supports '*' as wildcard.
fn filter_hosts(hosts: &[String], pattern: &str) -> Vec<String> {
    if pattern == "*" {
        return hosts.to_vec();
    }

    hosts
        .iter()
        .filter(|h| glob_match(pattern, h))
        .cloned()
        .collect()
}

/// Simple glob matching: '*' matches any sequence of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        // No wildcard — exact match
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) => {
                // First part must match at start
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }

    // Last part must match at end (unless pattern ends with *)
    if !pattern.ends_with('*') {
        return pos == text.len();
    }

    true
}

/// Build the system closure for a host and return the store path.
async fn build_host(flake: &str, host: &str) -> Result<String> {
    tracing::info!(host, "Building closure");

    let output = tokio::process::Command::new("nix")
        .args([
            "build",
            &format!(
                "{}#nixosConfigurations.{}.config.system.build.toplevel",
                flake, host
            ),
            "--print-out-paths",
            "--no-link",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context(format!("Failed to build closure for {}", host))?;

    if !output.status.success() {
        bail!(
            "Build failed for {}: {}",
            host,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let store_path = String::from_utf8(output.stdout)?.trim().to_string();
    if store_path.is_empty() {
        bail!("Build produced empty store path for {}", host);
    }

    Ok(store_path)
}

/// Push desired generation to the control plane.
async fn push_to_control_plane(cp_url: &str, host: &str, store_path: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/machines/{}/set-generation", cp_url, host);

    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "hash": store_path }))
        .send()
        .await
        .context(format!("Failed to reach control plane for {}", host))?;

    if !resp.status().is_success() {
        bail!(
            "Control plane returned {} for {}: {}",
            resp.status(),
            host,
            resp.text().await.unwrap_or_default()
        );
    }

    Ok(())
}

/// Deploy via SSH fallback: nix-copy-closure + switch-to-configuration.
async fn deploy_via_ssh(host: &str, store_path: &str) -> Result<()> {
    tracing::info!(host, store_path, "Copying closure via SSH");

    let copy_status = tokio::process::Command::new("nix-copy-closure")
        .args(["--to", &format!("root@{}", host), store_path])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context(format!("nix-copy-closure failed for {}", host))?;

    if !copy_status.success() {
        bail!("nix-copy-closure failed for {}", host);
    }

    tracing::info!(host, "Switching configuration");

    let switch_status = tokio::process::Command::new("ssh")
        .args([
            &format!("root@{}", host),
            &format!("{}/bin/switch-to-configuration", store_path),
            "switch",
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context(format!("SSH switch failed for {}", host))?;

    if !switch_status.success() {
        bail!("switch-to-configuration failed on {}", host);
    }

    Ok(())
}

pub async fn run(cp_url: &str, pattern: &str, flake: &str, dry_run: bool, ssh: bool) -> Result<()> {
    println!("Discovering hosts from {}...", flake);
    let all_hosts = discover_hosts(flake).await?;
    let targets = filter_hosts(&all_hosts, pattern);

    if targets.is_empty() {
        bail!(
            "No hosts match pattern '{}'. Available: {}",
            pattern,
            all_hosts.join(", ")
        );
    }

    println!(
        "Deploying to {} host(s): {}",
        targets.len(),
        targets.join(", ")
    );

    let mut results: HashMap<String, Result<String>> = HashMap::new();

    // Build all targets
    for host in &targets {
        print!("  Building {}... ", host);
        match build_host(flake, host).await {
            Ok(path) => {
                println!("{}", path);
                results.insert(host.clone(), Ok(path));
            }
            Err(e) => {
                println!("FAILED: {}", e);
                results.insert(host.clone(), Err(e));
            }
        }
    }

    if dry_run {
        println!("\n--- Dry run summary ---");
        for host in &targets {
            match results.get(host) {
                Some(Ok(path)) => println!("  {} -> {}", host, path),
                Some(Err(e)) => println!("  {} -> BUILD FAILED: {}", host, e),
                None => println!("  {} -> SKIPPED", host),
            }
        }
        println!("(No changes pushed. Remove --dry-run to deploy.)");
        return Ok(());
    }

    // Deploy successful builds
    let mut success_count = 0;
    let mut fail_count = 0;

    for host in &targets {
        if let Some(Ok(store_path)) = results.get(host) {
            if ssh {
                print!("  Deploying {} via SSH... ", host);
                match deploy_via_ssh(host, store_path).await {
                    Ok(()) => {
                        println!("OK");
                        success_count += 1;
                    }
                    Err(e) => {
                        println!("FAILED: {}", e);
                        fail_count += 1;
                    }
                }
            } else {
                print!("  Pushing {} to control plane... ", host);
                match push_to_control_plane(cp_url, host, store_path).await {
                    Ok(()) => {
                        println!("OK");
                        success_count += 1;
                    }
                    Err(e) => {
                        println!("FAILED: {}", e);
                        fail_count += 1;
                    }
                }
            }
        } else {
            fail_count += 1;
        }
    }

    println!(
        "\nDeploy complete: {} succeeded, {} failed",
        success_count, fail_count
    );

    if fail_count > 0 {
        bail!("{} host(s) failed", fail_count);
    }

    Ok(())
}

/// Deploy via the rollout API instead of direct SSH or per-host control plane push.
#[allow(clippy::too_many_arguments)]
pub async fn deploy_rollout(
    cp_url: &str,
    api_key: &str,
    generation_hash: &str,
    tags: &[String],
    hosts: &[String],
    strategy: &str,
    batch_sizes: Option<Vec<String>>,
    failure_threshold: &str,
    on_failure: &str,
    health_timeout: u64,
    wait: bool,
) -> Result<()> {
    let parsed_strategy = match strategy {
        "canary" => RolloutStrategy::Canary,
        "staged" => RolloutStrategy::Staged,
        "all-at-once" | "all_at_once" => RolloutStrategy::AllAtOnce,
        other => bail!(
            "Unknown strategy: {}. Use canary, staged, or all-at-once.",
            other
        ),
    };

    let parsed_on_failure = match on_failure {
        "pause" => OnFailure::Pause,
        "revert" => OnFailure::Revert,
        other => bail!("Unknown on-failure: {}. Use pause or revert.", other),
    };

    let target = if !tags.is_empty() {
        RolloutTarget::Tags(tags.to_vec())
    } else if !hosts.is_empty() {
        RolloutTarget::Hosts(hosts.to_vec())
    } else {
        bail!("Either --tag or --hosts must be specified for rollout deploy");
    };

    let request = CreateRolloutRequest {
        generation_hash: generation_hash.to_string(),
        cache_url: None,
        strategy: parsed_strategy,
        batch_sizes,
        failure_threshold: failure_threshold.to_string(),
        on_failure: parsed_on_failure,
        health_timeout: Some(health_timeout),
        target,
    };

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", api_key))
            .expect("invalid API key"),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("failed to build HTTP client");

    let url = format!("{}/api/v1/rollouts", cp_url);
    let resp = client
        .post(&url)
        .json(&request)
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

    let created: CreateRolloutResponse = resp
        .json()
        .await
        .context("Failed to parse rollout response")?;

    println!(
        "Rollout created: {} ({} machines in {} batches)",
        created.rollout_id,
        created.total_machines,
        created.batches.len(),
    );

    for batch in &created.batches {
        println!(
            "  Batch {}: {} machine(s) — {}",
            batch.batch_index,
            batch.machine_ids.len(),
            batch.machine_ids.join(", "),
        );
    }

    if wait {
        println!("\nWaiting for rollout to complete...");
        crate::rollout::wait_for_completion(cp_url, api_key, &created.rollout_id).await?;
    } else {
        println!(
            "\nRollout {} started. Use `nixfleet rollout status {}` to track progress.",
            created.rollout_id, created.rollout_id,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_glob_match_prefix() {
        assert!(glob_match("web*", "web-01"));
        assert!(glob_match("web*", "web-02"));
        assert!(!glob_match("web*", "dev-01"));
    }

    #[test]
    fn test_glob_match_suffix() {
        assert!(glob_match("*-01", "web-01"));
        assert!(glob_match("*-02", "web-02"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("web-01", "web-01"));
        assert!(!glob_match("web-01", "web-02"));
    }

    #[test]
    fn test_glob_match_middle() {
        assert!(glob_match("edge-*-test", "edge-01-test"));
        assert!(!glob_match("edge-*-test", "edge-01-prod"));
    }

    #[test]
    fn test_filter_hosts_all() {
        let hosts = vec!["web-01".into(), "dev-01".into(), "srv-01".into()];
        assert_eq!(filter_hosts(&hosts, "*"), hosts);
    }

    #[test]
    fn test_filter_hosts_pattern() {
        let hosts = vec!["web-01".into(), "web-02".into(), "dev-01".into()];
        let filtered = filter_hosts(&hosts, "web*");
        assert_eq!(filtered, vec!["web-01", "web-02"]);
    }

    #[test]
    fn test_filter_hosts_no_match() {
        let hosts = vec!["web-01".into(), "dev-01".into()];
        let filtered = filter_hosts(&hosts, "nonexistent*");
        assert!(filtered.is_empty());
    }
}

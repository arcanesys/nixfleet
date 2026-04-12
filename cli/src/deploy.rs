use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{
    CreateRolloutRequest, CreateRolloutResponse, OnFailure, RolloutStrategy, RolloutTarget,
};
use std::collections::HashMap;
use std::process::Stdio;

use crate::glob::filter_hosts;

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

/// Deploy via SSH fallback: nix-copy-closure + switch-to-configuration.
/// `ssh_target` overrides the hostname for SSH connections (e.g. "root@192.168.1.10").
async fn deploy_via_ssh(host: &str, store_path: &str, ssh_target: &str) -> Result<()> {
    tracing::info!(host, ssh_target, store_path, "Copying closure via SSH");

    let copy_status = tokio::process::Command::new("nix-copy-closure")
        .args(["--to", ssh_target, store_path])
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
            ssh_target,
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

pub async fn run(
    _client: &reqwest::Client,
    _cp_url: &str,
    pattern: &str,
    flake: &str,
    dry_run: bool,
    _ssh: bool,
    target_override: Option<&str>,
) -> Result<()> {
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

    // --target requires exactly one host
    if target_override.is_some() && targets.len() > 1 {
        bail!(
            "--target can only be used with a single host, but {} hosts matched pattern '{}'",
            targets.len(),
            pattern
        );
    }

    let mut results: HashMap<String, Result<String>> = HashMap::new();

    // Build all targets
    let build_bar = crate::display::ProgressContext::bar(targets.len() as u64, "Building closures");

    for host in &targets {
        match build_host(flake, host).await {
            Ok(path) => {
                tracing::info!(host, path = %crate::display::truncate_store_path(&path, 60), "built");
                results.insert(host.clone(), Ok(path));
            }
            Err(e) => {
                tracing::warn!(host, error = %e, "build failed");
                results.insert(host.clone(), Err(e));
            }
        }
        build_bar.inc(1);
    }
    build_bar.finish_and_clear();

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

    // Deploy successful builds via SSH
    let mut success_count = 0;
    let mut fail_count = 0;

    let deploy_bar = crate::display::ProgressContext::bar(targets.len() as u64, "Deploying via SSH");

    for host in &targets {
        if let Some(Ok(store_path)) = results.get(host) {
            let ssh_dest = match target_override {
                Some(t) => t.to_string(),
                None => format!("root@{}", host),
            };
            match deploy_via_ssh(host, store_path, &ssh_dest).await {
                Ok(()) => {
                    tracing::info!(host, "deployed");
                    success_count += 1;
                }
                Err(e) => {
                    tracing::warn!(host, error = %e, "deploy failed");
                    fail_count += 1;
                }
            }
        } else {
            fail_count += 1;
        }
        deploy_bar.inc(1);
    }
    deploy_bar.finish_and_clear();

    println!(
        "\nDeploy complete: {} succeeded, {} failed",
        success_count, fail_count
    );

    if fail_count > 0 {
        bail!("{} host(s) failed", fail_count);
    }

    Ok(())
}

/// Parse a strategy string into a RolloutStrategy enum.
pub fn parse_strategy(strategy: &str) -> Result<RolloutStrategy> {
    match strategy {
        "canary" => Ok(RolloutStrategy::Canary),
        "staged" => Ok(RolloutStrategy::Staged),
        "all-at-once" | "all_at_once" => Ok(RolloutStrategy::AllAtOnce),
        other => bail!(
            "Unknown strategy: {}. Use canary, staged, or all-at-once.",
            other
        ),
    }
}

/// Parse an on-failure string into an OnFailure enum.
pub fn parse_on_failure(on_failure: &str) -> Result<OnFailure> {
    match on_failure {
        "pause" => Ok(OnFailure::Pause),
        "revert" => Ok(OnFailure::Revert),
        other => bail!("Unknown on-failure: {}. Use pause or revert.", other),
    }
}

fn resolve_target(tags: &[String], hosts: &[String]) -> Result<RolloutTarget> {
    if !tags.is_empty() {
        Ok(RolloutTarget::Tags(tags.to_vec()))
    } else if !hosts.is_empty() {
        Ok(RolloutTarget::Hosts(hosts.to_vec()))
    } else {
        bail!("Either --tag or --hosts must be specified for rollout deploy")
    }
}

/// Deploy via the rollout API instead of direct SSH or per-host control plane push.
#[allow(clippy::too_many_arguments)]
pub async fn deploy_rollout(
    client: &reqwest::Client,
    cp_url: &str,
    release_id: &str,
    tags: &[String],
    hosts: &[String],
    strategy: &str,
    batch_sizes: Option<Vec<String>>,
    failure_threshold: &str,
    on_failure: &str,
    health_timeout: u64,
    wait: bool,
    cache_url: Option<&str>,
) -> Result<()> {
    let parsed_strategy = parse_strategy(strategy)?;
    let parsed_on_failure = parse_on_failure(on_failure)?;
    let target = resolve_target(tags, hosts)?;

    let request = CreateRolloutRequest {
        release_id: release_id.to_string(),
        cache_url: cache_url.map(|s| s.to_string()),
        strategy: parsed_strategy,
        batch_sizes,
        failure_threshold: failure_threshold.to_string(),
        on_failure: parsed_on_failure,
        health_timeout: Some(health_timeout),
        target,
    };

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
            crate::client::read_error_body(resp).await
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
        crate::rollout::wait_for_completion(client, cp_url, &created.rollout_id, None).await?;
    } else {
        println!(
            "\nRollout {} started. Use `nixfleet rollout status {}` to track progress.",
            created.rollout_id, created.rollout_id,
        );
    }

    Ok(())
}


// Glob-matching tests live in `cli/src/glob.rs`; the deploy module
// just consumes `filter_hosts` from there now.

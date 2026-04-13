use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{
    CreateRolloutRequest, CreateRolloutResponse, OnFailure, RolloutStrategy, RolloutTarget,
};
use std::collections::HashMap;
use std::process::Stdio;

use crate::display;
use crate::glob::filter_hosts;

/// Discover NixOS host names from the flake by evaluating nixosConfigurations attribute names.
async fn discover_hosts(flake: &str) -> Result<Vec<String>> {
    let mut cmd = tokio::process::Command::new("nix");
    cmd.args([
        "eval",
        &format!("{}#nixosConfigurations", flake),
        "--apply",
        "builtins.attrNames",
        "--json",
    ]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }

    let output = if display::passthrough_output() {
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .await
            .context("Failed to run nix eval for host discovery")?
    } else {
        display::run_cmd_async(&mut cmd, None)
            .await
            .context("Failed to run nix eval for host discovery")?
    };

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
async fn build_host(
    flake: &str,
    host: &str,
    window: Option<&mut display::RollingWindow>,
) -> Result<String> {
    tracing::info!(host, "building closure");

    let mut cmd = tokio::process::Command::new("nix");
    cmd.args([
        "build",
        &format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            flake, host
        ),
        "--print-out-paths",
        "--no-link",
    ]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }

    let output = if display::passthrough_output() {
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .await
            .context(format!("Failed to build closure for {}", host))?
    } else {
        display::run_cmd_async(&mut cmd, window)
            .await
            .context(format!("Failed to build closure for {}", host))?
    };

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
#[allow(clippy::needless_option_as_deref)]
async fn deploy_via_ssh(
    host: &str,
    store_path: &str,
    ssh_target: &str,
    mut window: Option<&mut display::RollingWindow>,
) -> Result<()> {
    tracing::info!(host, ssh_target, store_path, "copying closure via SSH");

    let mut copy_cmd = tokio::process::Command::new("nix-copy-closure");
    copy_cmd.args(["--to", ssh_target, store_path]);

    let copy_output = if display::passthrough_output() {
        let status = copy_cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context(format!("nix-copy-closure failed for {}", host))?;
        std::process::Output { status, stdout: vec![], stderr: vec![] }
    } else {
        display::run_cmd_async(&mut copy_cmd, window.as_deref_mut())
            .await
            .context(format!("nix-copy-closure failed for {}", host))?
    };

    if !copy_output.status.success() {
        let stderr = String::from_utf8_lossy(&copy_output.stderr);
        bail!("nix-copy-closure failed for {}: {}", host, stderr);
    }

    tracing::info!(host, "switching configuration");

    let mut switch_cmd = tokio::process::Command::new("ssh");
    switch_cmd.args([
        "-o",
        "BatchMode=yes",
        ssh_target,
        &format!("{}/bin/switch-to-configuration", store_path),
        "switch",
    ]);

    let switch_output = if display::passthrough_output() {
        let status = switch_cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context(format!("SSH switch failed for {}", host))?;
        std::process::Output { status, stdout: vec![], stderr: vec![] }
    } else {
        display::run_cmd_async(&mut switch_cmd, window.as_deref_mut())
            .await
            .context(format!("SSH switch failed for {}", host))?
    };

    if !switch_output.status.success() {
        let stderr = String::from_utf8_lossy(&switch_output.stderr);
        bail!("switch-to-configuration failed on {}: {}", host, stderr);
    }

    Ok(())
}

pub async fn run(
    _client: &reqwest::Client,
    _cp_url: &str,
    patterns: &[String],
    flake: &str,
    dry_run: bool,
    _ssh: bool,
    target_override: Option<&str>,
) -> Result<()> {
    println!("Discovering hosts from {}...", flake);
    let all_hosts = discover_hosts(flake).await?;
    let targets = filter_hosts(&all_hosts, patterns);

    if targets.is_empty() {
        bail!(
            "No hosts match pattern '{}'. Available: {}",
            patterns.join(","),
            all_hosts.join(", ")
        );
    }

    if target_override.is_some() && targets.len() > 1 {
        bail!(
            "--target can only be used with a single host, but {} hosts matched pattern '{}'",
            targets.len(),
            patterns.join(",")
        );
    }

    let mut results: HashMap<String, Result<String>> = HashMap::new();

    // Build all targets
    {
        let mut window = if display::use_progress() {
            Some(display::RollingWindow::new("building", targets.len() as u64))
        } else {
            None
        };

        for host in &targets {
            if let Some(ref mut w) = window {
                w.set_line_prefix(host);
            }
            match build_host(flake, host, window.as_mut().and_then(|w| w.for_output())).await {
                Ok(path) => {
                    tracing::info!(host, path = %display::truncate_store_path(&path, 60), "built");
                    results.insert(host.clone(), Ok(path));
                }
                Err(e) => {
                    tracing::warn!(host, error = %e, "build failed");
                    if let Some(ref mut w) = window {
                        w.mark_error();
                    }
                    results.insert(host.clone(), Err(e));
                }
            }
            if let Some(ref w) = window {
                w.inc();
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

    // Deploy successful builds via SSH
    let mut success_count = 0;
    let mut fail_count = 0;

    {
        let mut window = if display::use_progress() {
            Some(display::RollingWindow::new("deploying", targets.len() as u64))
        } else {
            None
        };

        for host in &targets {
            if let Some(ref mut w) = window {
                w.set_line_prefix(host);
            }
            if let Some(Ok(store_path)) = results.get(host) {
                let ssh_dest = match target_override {
                    Some(t) => t.to_string(),
                    None => format!("root@{}", host),
                };
                match deploy_via_ssh(host, store_path, &ssh_dest, window.as_mut().and_then(|w| w.for_output())).await {
                    Ok(()) => {
                        tracing::info!(host, "deployed");
                        success_count += 1;
                    }
                    Err(e) => {
                        tracing::warn!(host, error = %e, "deploy failed");
                        if let Some(ref mut w) = window {
                            w.mark_error();
                        }
                        fail_count += 1;
                    }
                }
            } else {
                fail_count += 1;
            }
            if let Some(ref w) = window {
                w.inc();
            }
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
        bail!("Either --tags or --hosts must be specified for rollout deploy")
    }
}

/// Deploy via the rollout API.
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

    let resp = crate::client::check_response(resp).await?;

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

use anyhow::{bail, Context, Result};
use nixfleet_types::rollout::{
    CreateRolloutRequest, CreateRolloutResponse, OnFailure, RolloutStrategy, RolloutTarget,
};
use std::collections::HashMap;
use std::time::Duration;

use crate::display;
use crate::glob::filter_hosts;
use crate::release::{build_host, discover_hosts};

/// Deploy via SSH fallback: nix-copy-closure + switch-to-configuration.
#[allow(clippy::needless_option_as_deref)]
async fn deploy_via_ssh(
    host: &str,
    store_path: &str,
    ssh_target: &str,
    mut window: Option<&mut display::RollingWindow>,
    oplog: &mut crate::oplog::OpLog,
) -> Result<()> {
    // nix-copy-closure
    let t = std::time::Instant::now();
    tracing::info!(host, ssh_target, store_path, "copying closure via SSH");

    let mut copy_cmd = tokio::process::Command::new("nix-copy-closure");
    copy_cmd.args(["--to", ssh_target, store_path]);

    let copy_output = if display::passthrough_output() {
        display::run_cmd_async_passthrough(&mut copy_cmd)
            .await
            .context(format!("nix-copy-closure failed for {}", host))?
    } else {
        display::run_cmd_async(&mut copy_cmd, window.as_deref_mut())
            .await
            .context(format!("nix-copy-closure failed for {}", host))?
    };
    oplog.log_output(
        &format!("nix-copy-closure {}", host),
        Some(host),
        &copy_output,
        t.elapsed(),
    );

    if !copy_output.status.success() {
        let stderr = String::from_utf8_lossy(&copy_output.stderr);
        bail!("nix-copy-closure failed for {}: {}", host, stderr);
    }

    // switch-to-configuration
    let t = std::time::Instant::now();
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
        display::run_cmd_async_passthrough(&mut switch_cmd)
            .await
            .context(format!("SSH switch failed for {}", host))?
    } else {
        display::run_cmd_async(&mut switch_cmd, window.as_deref_mut())
            .await
            .context(format!("SSH switch failed for {}", host))?
    };
    oplog.log_output(
        &format!("ssh switch-to-configuration {}", host),
        Some(host),
        &switch_output,
        t.elapsed(),
    );

    if !switch_output.status.success() {
        let stderr = String::from_utf8_lossy(&switch_output.stderr);
        bail!("switch-to-configuration failed on {}: {}", host, stderr);
    }

    Ok(())
}

pub async fn run(
    client: &reqwest::Client,
    cp_url: &str,
    patterns: &[String],
    flake: &str,
    dry_run: bool,
    _ssh: bool,
    target_override: Option<&str>,
) -> Result<()> {
    let mut oplog = crate::oplog::OpLog::new("deploy")?;

    println!("Discovering hosts from {}...", flake);
    let all_hosts = discover_hosts(flake, &mut oplog).await?;
    let targets = filter_hosts(&all_hosts, patterns);

    if targets.is_empty() {
        oplog.finish(false, Some("no hosts match pattern"));
        bail!(
            "No hosts match pattern '{}'. Available: {}",
            patterns.join(","),
            all_hosts.join(", ")
        );
    }

    if target_override.is_some() && targets.len() > 1 {
        oplog.finish(false, Some("--target with multiple hosts"));
        bail!(
            "--target can only be used with a single host, but {} hosts matched pattern '{}'",
            targets.len(),
            patterns.join(",")
        );
    }

    oplog.log_start("deploy", flake, &targets);

    let result = run_inner(
        client,
        cp_url,
        flake,
        &targets,
        dry_run,
        target_override,
        &mut oplog,
    )
    .await;

    match &result {
        Ok(()) => oplog.finish(true, None),
        Err(e) => oplog.finish(false, Some(&format!("{e:#}"))),
    }

    result
}

/// Inner implementation for `run`, split out so oplog can wrap the result.
async fn run_inner(
    client: &reqwest::Client,
    cp_url: &str,
    flake: &str,
    targets: &[String],
    dry_run: bool,
    target_override: Option<&str>,
    oplog: &mut crate::oplog::OpLog,
) -> Result<()> {
    let mut results: HashMap<String, Result<String>> = HashMap::new();

    // Build all targets
    {
        let mut window = if display::use_progress() {
            Some(display::RollingWindow::new(
                "building",
                targets.len() as u64,
            ))
        } else {
            None
        };

        for host in targets {
            if let Some(ref mut w) = window {
                w.set_line_prefix(host);
            }
            match build_host(
                flake,
                host,
                window.as_mut().and_then(|w| w.for_output()),
                oplog,
            )
            .await
            {
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
        for host in targets {
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
            Some(display::RollingWindow::new(
                "deploying",
                targets.len() as u64,
            ))
        } else {
            None
        };

        for host in targets {
            if let Some(ref mut w) = window {
                w.set_line_prefix(host);
            }
            if let Some(Ok(store_path)) = results.get(host) {
                let ssh_dest = match target_override {
                    Some(t) => t.to_string(),
                    None => format!("root@{}", host),
                };
                match deploy_via_ssh(
                    host,
                    store_path,
                    &ssh_dest,
                    window.as_mut().and_then(|w| w.for_output()),
                    oplog,
                )
                .await
                {
                    Ok(()) => {
                        tracing::info!(host, "deployed");
                        // Notify the CP of the deployed store path so it
                        // tracks desired_generation and shows the machine
                        // in sync once the agent confirms.
                        if let Err(e) = notify_deploy_on_cp(client, cp_url, host, store_path).await
                        {
                            tracing::debug!(host, error = %e, "could not notify CP of deploy (CP may be unavailable)");
                        }
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
        bail!("{} host(s) failed to deploy", fail_count);
    }
    Ok(())
}

/// Best-effort: notify the CP of a successful SSH deploy.
/// Sets the machine's desired generation to the deployed store path so
/// the CP shows the machine in sync once the agent confirms.
async fn notify_deploy_on_cp(
    client: &reqwest::Client,
    cp_url: &str,
    machine_id: &str,
    store_path: &str,
) -> Result<()> {
    let url = format!("{}/api/v1/machines/{}/notify-deploy", cp_url, machine_id);
    let body = serde_json::json!({ "store_path": store_path });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("failed to reach control plane")?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!("CP returned {}", status)
    }
}

// ==========================================================================
// Rollout deployment (non-SSH path, goes through control plane)
// ==========================================================================

pub fn parse_strategy(strategy: &str) -> Result<RolloutStrategy> {
    match strategy {
        "canary" => Ok(RolloutStrategy::Canary),
        "staged" => Ok(RolloutStrategy::Staged),
        "all-at-once" | "all_at_once" => Ok(RolloutStrategy::AllAtOnce),
        _ => bail!(
            "Unknown strategy '{}'. Use canary, staged, or all-at-once.",
            strategy
        ),
    }
}

pub fn parse_on_failure(on_failure: &str) -> Result<OnFailure> {
    match on_failure {
        "pause" => Ok(OnFailure::Pause),
        "revert" => Ok(OnFailure::Revert),
        _ => bail!("Unknown on-failure '{}'. Use pause or revert.", on_failure),
    }
}

fn resolve_target(tags: &[String], hosts: &[String]) -> Result<RolloutTarget> {
    if !tags.is_empty() {
        Ok(RolloutTarget::Tags(tags.to_vec()))
    } else if !hosts.is_empty() {
        Ok(RolloutTarget::Hosts(hosts.to_vec()))
    } else {
        bail!("Either --tags or --hosts must be provided for rollout deploy");
    }
}

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
    wait_timeout: u64,
) -> Result<()> {
    let target = resolve_target(tags, hosts)?;
    let strategy = parse_strategy(strategy)?;
    let on_failure = parse_on_failure(on_failure)?;

    let body = CreateRolloutRequest {
        release_id: release_id.to_string(),
        cache_url: cache_url.map(|s| s.to_string()),
        strategy,
        batch_sizes,
        failure_threshold: failure_threshold.to_string(),
        on_failure,
        health_timeout: Some(health_timeout),
        target,
    };

    let resp = client
        .post(format!("{}/api/v1/rollouts", cp_url))
        .json(&body)
        .send()
        .await
        .context("failed to create rollout")?;

    let resp = crate::client::check_response(resp).await?;
    let created: CreateRolloutResponse = resp
        .json()
        .await
        .context("failed to parse rollout response")?;

    println!(
        "Rollout {} created ({} batches)",
        created.rollout_id,
        created.batches.len()
    );

    if wait {
        let timeout = if wait_timeout == 0 {
            Some(Duration::ZERO)
        } else {
            Some(Duration::from_secs(wait_timeout))
        };
        crate::rollout::wait_for_completion(client, cp_url, &created.rollout_id, timeout).await?;
    }

    Ok(())
}

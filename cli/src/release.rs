use crate::display;
use crate::glob::filter_hosts;
use anyhow::{Context, Result};
use console::style;
use nixfleet_types::release::{
    CreateReleaseRequest, CreateReleaseResponse, Release, ReleaseDiff, ReleaseEntry,
};
use reqwest::Client;
use std::process::Command;

/// Discover all nixosConfigurations host names from a flake.
fn discover_hosts(flake: &str, oplog: &mut crate::oplog::OpLog) -> Result<Vec<String>> {
    let t = std::time::Instant::now();
    let mut cmd = Command::new("nix");
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
        cmd.stderr(std::process::Stdio::inherit())
            .output()
            .context("failed to run nix eval")?
    } else {
        display::run_cmd(&mut cmd, None).context("failed to run nix eval")?
    };
    oplog.log_output("nix eval discover hosts", None, &output, t.elapsed());
    if !output.status.success() {
        anyhow::bail!(
            "nix eval failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let hosts: Vec<String> =
        serde_json::from_slice(&output.stdout).context("failed to parse nix eval output")?;
    Ok(hosts)
}

/// Detect platform for a host.
fn detect_platform(flake: &str, hostname: &str, oplog: &mut crate::oplog::OpLog) -> Result<String> {
    let t = std::time::Instant::now();
    let mut cmd = Command::new("nix");
    cmd.args([
        "eval",
        &format!("{}#nixosConfigurations.{}.pkgs.system", flake, hostname),
        "--raw",
    ]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }
    let output = if display::passthrough_output() {
        cmd.stderr(std::process::Stdio::inherit())
            .output()
            .context("failed to detect platform")?
    } else {
        display::run_cmd(&mut cmd, None).context("failed to detect platform")?
    };
    oplog.log_output(
        &format!("nix eval platform {}", hostname),
        Some(hostname),
        &output,
        t.elapsed(),
    );
    if !output.status.success() {
        anyhow::bail!(
            "failed to detect platform for {}: {}",
            hostname,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Detect tags for a host (best-effort).
fn detect_tags(flake: &str, hostname: &str) -> Vec<String> {
    let mut cmd = Command::new("nix");
    cmd.args([
        "eval",
        &format!(
            "{}#nixosConfigurations.{}.config.services.nixfleet-agent.tags",
            flake, hostname
        ),
        "--json",
    ]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }
    let output = if display::passthrough_output() {
        cmd.stderr(std::process::Stdio::inherit()).output()
    } else {
        display::run_cmd(&mut cmd, None)
    };
    match output {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => vec![],
    }
}

/// Build a host's toplevel closure.
fn build_host(
    flake: &str,
    hostname: &str,
    window: Option<&mut display::RollingWindow>,
    oplog: &mut crate::oplog::OpLog,
) -> Result<String> {
    let t = std::time::Instant::now();
    tracing::info!(hostname, "building closure");
    let mut cmd = Command::new("nix");
    cmd.args([
        "build",
        &format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            flake, hostname
        ),
        "--print-out-paths",
        "--no-link",
    ]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }
    let output = if display::passthrough_output() {
        cmd.stderr(std::process::Stdio::inherit())
            .output()
            .context("failed to run nix build")?
    } else {
        display::run_cmd(&mut cmd, window).context("failed to run nix build")?
    };
    oplog.log_output(
        &format!("nix build {}", hostname),
        Some(hostname),
        &output,
        t.elapsed(),
    );
    if !output.status.success() {
        anyhow::bail!(
            "nix build failed for {}: {}",
            hostname,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Evaluate a host's store path without building.
fn eval_host(flake: &str, hostname: &str, oplog: &mut crate::oplog::OpLog) -> Result<String> {
    let t = std::time::Instant::now();
    let attr = format!(
        "{}#nixosConfigurations.{}.config.system.build.toplevel.outPath",
        flake, hostname
    );
    let mut cmd = std::process::Command::new("nix");
    cmd.args(["eval", &attr, "--raw"]);
    let output = display::run_cmd(&mut cmd, None)?;
    oplog.log_output(
        &format!("nix eval {}", hostname),
        Some(hostname),
        &output,
        t.elapsed(),
    );
    if !output.status.success() {
        anyhow::bail!(
            "nix eval failed for {}: {}",
            hostname,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Copy a store path to a Nix binary cache (S3, SSH, HTTP, etc.).
fn nix_copy_to(
    cache_url: &str,
    store_path: &str,
    hostname: &str,
    window: Option<&mut display::RollingWindow>,
    oplog: &mut crate::oplog::OpLog,
) -> Result<()> {
    let t = std::time::Instant::now();
    tracing::info!(store_path, dest = cache_url, "copying closure");
    let mut cmd = Command::new("nix");
    cmd.args(["copy", "--to", cache_url, store_path]);
    if display::quiet_subprocess() {
        cmd.arg("--quiet");
    }
    let output = if display::passthrough_output() {
        let status = cmd
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("failed to run nix copy --to")?;
        std::process::Output {
            status,
            stdout: vec![],
            stderr: vec![],
        }
    } else {
        display::run_cmd(&mut cmd, window).context("failed to run nix copy --to")?
    };
    oplog.log_output(
        &format!("nix copy --to {} {}", cache_url, hostname),
        Some(hostname),
        &output,
        t.elapsed(),
    );
    if !output.status.success() {
        anyhow::bail!(
            "nix copy --to {} failed for {}: {}",
            cache_url,
            store_path,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Copy a closure to a remote host via SSH.
fn copy_to_host(
    hostname: &str,
    store_path: &str,
    window: Option<&mut display::RollingWindow>,
    oplog: &mut crate::oplog::OpLog,
) -> Result<()> {
    let t = std::time::Instant::now();
    tracing::info!(store_path, dest = hostname, "copying closure");
    let mut cmd = Command::new("nix-copy-closure");
    cmd.args(["--to", &format!("root@{}", hostname), store_path]);
    let output = if display::passthrough_output() {
        let status = cmd
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("failed to run nix-copy-closure")?;
        std::process::Output {
            status,
            stdout: vec![],
            stderr: vec![],
        }
    } else {
        display::run_cmd(&mut cmd, window).context("failed to run nix-copy-closure")?
    };
    oplog.log_output(
        &format!("nix-copy-closure {}", hostname),
        Some(hostname),
        &output,
        t.elapsed(),
    );
    if !output.status.success() {
        anyhow::bail!("nix-copy-closure failed for {}", hostname);
    }
    Ok(())
}

/// Resolve the flake's git revision.
fn flake_revision(flake: &str) -> Option<String> {
    let mut cmd = Command::new("nix");
    cmd.args(["flake", "metadata", flake, "--json"]);
    let output = if display::passthrough_output() {
        cmd.stderr(std::process::Stdio::inherit()).output().ok()?
    } else {
        display::run_cmd(&mut cmd, None).ok()?
    };
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    metadata.get("revision")?.as_str().map(|s| s.to_string())
}

/// Run a push hook command for a store path, optionally on a remote host via SSH.
pub fn run_push_hook(
    push_to_host: Option<&str>,
    hook_cmd: &str,
    store_path: &str,
    hostname: Option<&str>,
    window: Option<&mut display::RollingWindow>,
    oplog: Option<&mut crate::oplog::OpLog>,
) -> Result<()> {
    let t = std::time::Instant::now();
    let cmd_str = hook_cmd.replace("{}", store_path);
    tracing::info!(cmd = %cmd_str, "running push hook");
    let mut cmd = match push_to_host {
        Some(host) => {
            let mut c = Command::new("ssh");
            c.args(["-o", "BatchMode=yes", host, &cmd_str]);
            c
        }
        None => {
            let mut c = Command::new("sh");
            c.args(["-c", &cmd_str]);
            c
        }
    };
    if display::passthrough_output() {
        let status = cmd
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .status()
            .context("failed to run push hook")?;
        if let Some(oplog) = oplog {
            let output = std::process::Output {
                status,
                stdout: vec![],
                stderr: vec![],
            };
            oplog.log_output(
                &format!("push-hook {}", hostname.unwrap_or("local")),
                hostname,
                &output,
                t.elapsed(),
            );
        }
        if !status.success() {
            anyhow::bail!("push hook failed: {}", cmd_str);
        }
        return Ok(());
    }
    let output = display::run_cmd(&mut cmd, window).context("failed to run push hook")?;
    if let Some(oplog) = oplog {
        oplog.log_output(
            &format!("push-hook {}", hostname.unwrap_or("local")),
            hostname,
            &output,
            t.elapsed(),
        );
    }
    if !output.status.success() {
        anyhow::bail!("push hook failed: {}", cmd_str);
    }
    Ok(())
}

/// Extract SSH host from a URL like ssh://root@host or ssh://host.
pub fn extract_ssh_host(url: &str) -> Option<String> {
    url.strip_prefix("ssh://")
        .map(|rest| rest.trim_end_matches('/').to_string())
}

/// `nixfleet release create`
// CRUD function arguments map directly to table columns; refactoring is busywork
#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    base_url: &str,
    flake: &str,
    host_patterns: &[String],
    push_to: Option<&str>,
    push_hook: Option<&str>,
    copy: bool,
    cache_url: Option<&str>,
    dry_run: bool,
    eval_only: bool,
) -> Result<Option<String>> {
    let mut oplog = crate::oplog::OpLog::new("release-create")?;

    tracing::info!("discovering hosts");
    let all_hosts = discover_hosts(flake, &mut oplog)?;
    let hosts = filter_hosts(&all_hosts, host_patterns);
    if hosts.is_empty() {
        anyhow::bail!("no hosts match pattern '{}'", host_patterns.join(","));
    }
    tracing::info!(count = hosts.len(), "found hosts");

    oplog.log_start("release_create", flake, &hosts);

    let result = create_inner(
        client, base_url, flake, &hosts, push_to, push_hook, copy, cache_url, dry_run, eval_only,
        &mut oplog,
    )
    .await;

    match &result {
        Ok(_) => oplog.finish(true, None),
        Err(e) => oplog.finish(false, Some(&format!("{e:#}"))),
    }

    result
}

/// Inner implementation for `create`, split out so oplog can wrap the result.
#[allow(clippy::too_many_arguments)]
async fn create_inner(
    client: &Client,
    base_url: &str,
    flake: &str,
    hosts: &[String],
    push_to: Option<&str>,
    push_hook: Option<&str>,
    copy: bool,
    cache_url: Option<&str>,
    dry_run: bool,
    eval_only: bool,
    oplog: &mut crate::oplog::OpLog,
) -> Result<Option<String>> {
    // Build all hosts
    let mut entries = Vec::new();
    {
        let mut window = if display::use_progress() {
            Some(display::RollingWindow::new("building", hosts.len() as u64))
        } else {
            None
        };

        for hostname in hosts {
            if let Some(ref mut w) = window {
                w.set_line_prefix(hostname);
            }
            let platform = detect_platform(flake, hostname, oplog)?;
            let tags = detect_tags(flake, hostname);
            let build_result = if eval_only {
                eval_host(flake, hostname, oplog)
            } else {
                build_host(
                    flake,
                    hostname,
                    window.as_mut().and_then(|w| w.for_output()),
                    oplog,
                )
            };
            match build_result {
                Ok(store_path) => {
                    if platform.contains("darwin") {
                        tracing::info!(
                            hostname,
                            "Darwin host — built and cached but not deployable via agent"
                        );
                    }
                    entries.push(ReleaseEntry {
                        hostname: hostname.clone(),
                        store_path,
                        platform,
                        tags,
                    });
                }
                Err(e) => {
                    if let Some(ref mut w) = window {
                        w.mark_error();
                    }
                    return Err(e);
                }
            }
            if let Some(ref w) = window {
                w.inc();
            }
        }
    }

    // Distribute
    if !eval_only {
        if let Some(push_url) = push_to {
            let mut pushed: std::collections::HashSet<String> = std::collections::HashSet::new();
            let unique_count = entries
                .iter()
                .map(|e| &e.store_path)
                .collect::<std::collections::HashSet<_>>()
                .len();
            {
                let mut window = if display::use_progress() {
                    Some(display::RollingWindow::new("pushing", unique_count as u64))
                } else {
                    None
                };

                for entry in &entries {
                    if pushed.insert(entry.store_path.clone()) {
                        if let Err(e) = nix_copy_to(
                            push_url,
                            &entry.store_path,
                            &entry.hostname,
                            window.as_mut().and_then(|w| w.for_output()),
                            oplog,
                        ) {
                            if let Some(ref mut w) = window {
                                w.mark_error();
                            }
                            return Err(e);
                        }
                        if let Some(ref w) = window {
                            w.inc();
                        }
                    }
                }
            }

            if let Some(hook) = push_hook {
                let remote_host = extract_ssh_host(push_url);
                tracing::info!(
                    host = remote_host.as_deref().unwrap_or("localhost"),
                    "running push hook"
                );
                let mut window = if display::use_progress() {
                    Some(display::RollingWindow::new(
                        "push-hook",
                        entries.len() as u64,
                    ))
                } else {
                    None
                };
                for entry in &entries {
                    if let Some(ref mut w) = window {
                        w.set_line_prefix(&entry.hostname);
                    }
                    run_push_hook(
                        remote_host.as_deref(),
                        hook,
                        &entry.store_path,
                        Some(&entry.hostname),
                        window.as_mut().and_then(|w| w.for_output()),
                        Some(oplog),
                    )?;
                    if let Some(ref w) = window {
                        w.inc();
                    }
                }
            }
        } else if let Some(hook) = push_hook {
            tracing::info!("running push hook locally");
            let mut window = if display::use_progress() {
                Some(display::RollingWindow::new(
                    "push-hook",
                    entries.len() as u64,
                ))
            } else {
                None
            };
            for entry in &entries {
                if let Some(ref mut w) = window {
                    w.set_line_prefix(&entry.hostname);
                }
                run_push_hook(
                    None,
                    hook,
                    &entry.store_path,
                    Some(&entry.hostname),
                    window.as_mut().and_then(|w| w.for_output()),
                    Some(oplog),
                )?;
                if let Some(ref w) = window {
                    w.inc();
                }
            }
        } else if copy {
            {
                let mut window = if display::use_progress() {
                    Some(display::RollingWindow::new("copying", entries.len() as u64))
                } else {
                    None
                };

                for entry in &entries {
                    if let Some(ref mut w) = window {
                        w.set_line_prefix(&entry.hostname);
                    }
                    if entry.platform.contains("darwin") {
                        tracing::info!(hostname = %entry.hostname, "skipping Darwin host");
                        if let Some(ref w) = window {
                            w.inc();
                        }
                        continue;
                    }
                    let copy_result = copy_to_host(
                        &entry.hostname,
                        &entry.store_path,
                        window.as_mut().and_then(|w| w.for_output()),
                        oplog,
                    );
                    if let Err(e) = copy_result {
                        tracing::warn!(hostname = %entry.hostname, error = %e, "failed to copy");
                        if let Some(ref mut w) = window {
                            w.mark_error();
                        }
                    }
                    if let Some(ref w) = window {
                        w.inc();
                    }
                }
            }
        }
    } // close if !eval_only

    // Print summary
    let manifest_rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            vec![
                entry.hostname.clone(),
                entry.platform.clone(),
                entry.store_path.clone(),
            ]
        })
        .collect();

    println!("\nRelease manifest:");
    display::print_table(&["HOST", "PLATFORM", "STORE PATH"], &manifest_rows);

    if dry_run {
        println!("\n(dry-run: not registering with control plane)");
        return Ok(None);
    }

    // Register with CP
    let flake_rev = flake_revision(flake);
    let req = CreateReleaseRequest {
        flake_ref: Some(flake.to_string()),
        flake_rev,
        cache_url: cache_url.map(|s| s.to_string()),
        entries,
    };

    let resp = client
        .post(format!("{}/api/v1/releases", base_url))
        .json(&req)
        .send()
        .await
        .context("failed to POST release")?;

    let resp = crate::client::check_response(resp).await?;

    let release_resp: CreateReleaseResponse = resp.json().await?;
    println!(
        "\nRelease {} created ({} hosts)",
        release_resp.id, release_resp.host_count
    );
    Ok(Some(release_resp.id))
}

/// `nixfleet release list`
pub async fn list(
    client: &Client,
    base_url: &str,
    limit: u32,
    host: Option<&str>,
    json: bool,
) -> Result<()> {
    let mut url = format!("{}/api/v1/releases?limit={}", base_url, limit);
    if let Some(h) = host {
        url.push_str(&format!("&host={}", h));
    }
    let resp = client
        .get(url)
        .send()
        .await
        .context("failed to GET releases")?;

    let resp = crate::client::check_response(resp).await?;

    let releases: Vec<Release> = resp.json().await?;
    if releases.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No releases found.");
        }
        return Ok(());
    }

    let rows: Vec<Vec<String>> = releases
        .iter()
        .map(|r| {
            let rev = r.flake_rev.as_deref().unwrap_or("-");
            let rev_short = if rev.len() > 8 { &rev[..8] } else { rev };
            vec![
                r.id.clone(),
                rev_short.to_string(),
                r.host_count.to_string(),
                r.created_at.format("%Y-%m-%d %H:%M").to_string(),
                r.created_by.clone(),
            ]
        })
        .collect();

    display::print_list(
        json,
        &["ID", "REVISION", "HOSTS", "CREATED", "BY"],
        &rows,
        &releases,
    );

    Ok(())
}

/// `nixfleet release show`
pub async fn show(client: &Client, base_url: &str, release_id: &str, json: bool) -> Result<()> {
    let resp = client
        .get(format!("{}/api/v1/releases/{}", base_url, release_id))
        .send()
        .await
        .context("failed to GET release")?;

    let resp = crate::client::check_response(resp).await?;

    let release: Release = resp.json().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&release)?);
        return Ok(());
    }

    display::print_detail(&[
        ("Release", release.id.clone()),
        (
            "Flake ref",
            release.flake_ref.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "Flake rev",
            release.flake_rev.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "Cache URL",
            release.cache_url.as_deref().unwrap_or("-").to_string(),
        ),
        ("Hosts", release.host_count.to_string()),
        (
            "Created",
            release.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ),
        ("By", release.created_by.clone()),
    ]);

    println!();
    let entry_rows: Vec<Vec<String>> = release
        .entries
        .iter()
        .map(|e| {
            let tags = if e.tags.is_empty() {
                "-".to_string()
            } else {
                e.tags.join(", ")
            };
            vec![
                e.hostname.clone(),
                e.platform.clone(),
                display::truncate_store_path(&e.store_path, 50),
                tags,
            ]
        })
        .collect();

    display::print_table(&["HOST", "PLATFORM", "STORE PATH", "TAGS"], &entry_rows);

    Ok(())
}

/// `nixfleet release diff`
pub async fn diff(
    client: &Client,
    base_url: &str,
    id_a: &str,
    id_b: &str,
    json: bool,
) -> Result<()> {
    let resp = client
        .get(format!(
            "{}/api/v1/releases/{}/diff/{}",
            base_url, id_a, id_b
        ))
        .send()
        .await
        .context("failed to GET release diff")?;

    let resp = crate::client::check_response(resp).await?;

    let diff: ReleaseDiff = resp.json().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
        return Ok(());
    }

    if !diff.added.is_empty() {
        println!("Added hosts:");
        for host in &diff.added {
            println!("  {} {}", style("+").green(), host);
        }
    }
    if !diff.removed.is_empty() {
        println!("Removed hosts:");
        for host in &diff.removed {
            println!("  {} {}", style("-").red(), host);
        }
    }
    if !diff.changed.is_empty() {
        println!("Changed hosts:");
        for entry in &diff.changed {
            println!("  {} {}", style("~").yellow(), entry.hostname);
            println!("    old: {}", entry.old_store_path);
            println!("    new: {}", entry.new_store_path);
        }
    }
    if !diff.unchanged.is_empty() {
        println!("Unchanged: {}", diff.unchanged.join(", "));
    }
    if diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty() {
        println!("No differences.");
    }
    Ok(())
}

/// `nixfleet release delete`
///
/// 204 → exit 0 with confirmation message.
/// 409 → exit 1 with explanatory message (release still referenced by a rollout).
/// 404 → exit 1 with explanatory message.
/// other non-2xx → exit 1 with the response body.
pub async fn delete(client: &Client, base_url: &str, release_id: &str) -> Result<()> {
    let resp = client
        .delete(format!("{}/api/v1/releases/{}", base_url, release_id))
        .send()
        .await
        .context("failed to DELETE release")?;

    let status = resp.status();
    if status.as_u16() == 204 || status.is_success() {
        println!("Release {release_id} deleted");
        return Ok(());
    }
    if status.as_u16() == 409 {
        anyhow::bail!("Release {release_id} cannot be deleted: still referenced by a rollout");
    }
    if status.as_u16() == 404 {
        anyhow::bail!("Release {release_id} not found");
    }
    let body = crate::client::read_error_body(resp).await;
    anyhow::bail!("failed to delete release: {} {}", status, body);
}

use anyhow::{Context, Result};
use console::style;
use nixfleet_types::release::{
    CreateReleaseRequest, CreateReleaseResponse, Release, ReleaseDiff, ReleaseEntry,
};
use reqwest::Client;
use std::process::Command;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::display;
use crate::glob::filter_hosts;

/// Discover all nixosConfigurations host names from a flake.
fn discover_hosts(flake: &str) -> Result<Vec<String>> {
    let output = Command::new("nix")
        .args([
            "eval",
            &format!("{}#nixosConfigurations", flake),
            "--apply",
            "builtins.attrNames",
            "--json",
            "--quiet",
        ])
        .output()
        .context("failed to run nix eval")?;
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
fn detect_platform(flake: &str, hostname: &str) -> Result<String> {
    let output = Command::new("nix")
        .args([
            "eval",
            &format!("{}#nixosConfigurations.{}.pkgs.system", flake, hostname),
            "--raw",
            "--quiet",
        ])
        .output()
        .context("failed to detect platform")?;
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
    let output = Command::new("nix")
        .args([
            "eval",
            &format!(
                "{}#nixosConfigurations.{}.config.services.nixfleet-agent.tags",
                flake, hostname
            ),
            "--json",
            "--quiet",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => vec![],
    }
}

/// Build a host's toplevel closure.
fn build_host(flake: &str, hostname: &str) -> Result<String> {
    tracing::info!(hostname, "building closure");
    let output = Command::new("nix")
        .args([
            "build",
            &format!(
                "{}#nixosConfigurations.{}.config.system.build.toplevel",
                flake, hostname
            ),
            "--print-out-paths",
            "--no-link",
            "--quiet",
        ])
        .output()
        .context("failed to run nix build")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix build failed for {}: {}",
            hostname,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Copy a store path to a Nix binary cache (S3, SSH, HTTP, etc.).
fn nix_copy_to(cache_url: &str, store_path: &str) -> Result<()> {
    tracing::info!(store_path, dest = cache_url, "copying closure");
    let output = Command::new("nix")
        .args(["copy", "--to", cache_url, store_path, "--quiet"])
        .output()
        .context("failed to run nix copy --to")?;
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
fn copy_to_host(hostname: &str, store_path: &str) -> Result<()> {
    tracing::info!(store_path, dest = hostname, "copying closure");
    let output = Command::new("nix-copy-closure")
        .args(["--to", &format!("root@{}", hostname), store_path])
        .output()
        .context("failed to run nix-copy-closure")?;
    if !output.status.success() {
        anyhow::bail!(
            "nix-copy-closure failed for {}: {}",
            hostname,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Resolve the flake's git revision.
fn flake_revision(flake: &str) -> Option<String> {
    let output = Command::new("nix")
        .args(["flake", "metadata", flake, "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    metadata.get("revision")?.as_str().map(|s| s.to_string())
}

/// `nixfleet release create`
/// Run a push hook command for a store path, optionally on a remote host via SSH.
pub fn run_push_hook(push_to_host: Option<&str>, hook_cmd: &str, store_path: &str) -> Result<()> {
    let cmd = hook_cmd.replace("{}", store_path);
    tracing::info!(cmd = %cmd, "running push hook");
    let status = match push_to_host {
        Some(host) => Command::new("ssh")
            .args([host, &cmd])
            .status()
            .context("failed to run push hook via SSH")?,
        None => Command::new("sh")
            .args(["-c", &cmd])
            .status()
            .context("failed to run push hook")?,
    };
    if !status.success() {
        anyhow::bail!("push hook failed: {}", cmd);
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
    hosts_pattern: &str,
    push_to: Option<&str>,
    push_hook: Option<&str>,
    copy: bool,
    cache_url: Option<&str>,
    dry_run: bool,
) -> Result<Option<String>> {
    tracing::info!("discovering hosts");
    let all_hosts = discover_hosts(flake)?;
    let hosts = filter_hosts(&all_hosts, hosts_pattern);
    if hosts.is_empty() {
        anyhow::bail!("no hosts match pattern '{}'", hosts_pattern);
    }
    tracing::info!(count = hosts.len(), "found hosts");

    // Build all hosts
    let mut entries = Vec::new();
    {
        let span = tracing::info_span!("building");
        span.pb_set_length(hosts.len() as u64);
        span.pb_set_style(&crate::display::progress_style());
        for hostname in &hosts {
            let _guard = span.enter();
            let platform = detect_platform(flake, hostname)?;
            let tags = detect_tags(flake, hostname);
            let store_path = build_host(flake, hostname)?;

            if platform.contains("darwin") {
                tracing::info!(hostname, "Darwin host — built and cached but not deployable via agent");
            }

            entries.push(ReleaseEntry {
                hostname: hostname.clone(),
                store_path,
                platform,
                tags,
            });
            span.pb_inc(1);
        }
    }

    // Distribute
    if let Some(push_url) = push_to {
        let mut pushed: std::collections::HashSet<String> = std::collections::HashSet::new();
        let unique_count = entries
            .iter()
            .map(|e| &e.store_path)
            .collect::<std::collections::HashSet<_>>()
            .len();
        {
            let span = tracing::info_span!("pushing", dest = %push_url);
            span.pb_set_length(unique_count as u64);
        span.pb_set_style(&crate::display::progress_style());
            for entry in &entries {
                let _guard = span.enter();
                if pushed.insert(entry.store_path.clone()) {
                    nix_copy_to(push_url, &entry.store_path)?;
                    span.pb_inc(1);
                }
            }
        }

        // Run push hook on the remote host if specified
        if let Some(hook) = push_hook {
            let remote_host = extract_ssh_host(push_url);
            tracing::info!(
                host = remote_host.as_deref().unwrap_or("localhost"),
                "running push hook"
            );
            for entry in &entries {
                run_push_hook(remote_host.as_deref(), hook, &entry.store_path)?;
            }
        }
    } else if let Some(hook) = push_hook {
        // Hook without push-to: run locally
        tracing::info!("running push hook locally");
        for entry in &entries {
            run_push_hook(None, hook, &entry.store_path)?;
        }
    } else if copy {
        {
            let span = tracing::info_span!("copying");
            span.pb_set_length(entries.len() as u64);
        span.pb_set_style(&crate::display::progress_style());
            for entry in &entries {
                let _guard = span.enter();
                if entry.platform.contains("darwin") {
                    tracing::info!(hostname = %entry.hostname, "skipping Darwin host");
                    span.pb_inc(1);
                    continue;
                }
                if let Err(e) = copy_to_host(&entry.hostname, &entry.store_path) {
                    tracing::warn!(hostname = %entry.hostname, error = %e, "failed to copy");
                }
                span.pb_inc(1);
            }
        }
    }

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
pub async fn list(client: &Client, base_url: &str, limit: u32, json: bool) -> Result<()> {
    let resp = client
        .get(format!("{}/api/v1/releases?limit={}", base_url, limit))
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
            release
                .flake_ref
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Flake rev",
            release
                .flake_rev
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Cache URL",
            release
                .cache_url
                .as_deref()
                .unwrap_or("-")
                .to_string(),
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

    display::print_table(
        &["HOST", "PLATFORM", "STORE PATH", "TAGS"],
        &entry_rows,
    );

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

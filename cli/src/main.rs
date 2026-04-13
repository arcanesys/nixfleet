use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::Path;
use std::process::Stdio;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod client;
mod config;
mod deploy;
mod display;
mod glob;
mod host;
mod machines;
mod oplog;
mod release;
mod rollout;
mod status;

#[derive(Parser)]
#[command(name = "nixfleet", about = "NixFleet fleet management CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Control plane URL
    #[arg(long, global = true, default_value = "http://localhost:8080")]
    control_plane_url: String,

    /// API key for control plane authentication
    #[arg(long, global = true, default_value = "")]
    api_key: String,

    /// Client certificate for mTLS authentication
    #[arg(long, global = true, default_value = "")]
    client_cert: String,

    /// Client key for mTLS authentication
    #[arg(long, global = true, default_value = "")]
    client_key: String,

    /// CA certificate for TLS verification (optional, uses system trust store if omitted)
    #[arg(long, global = true, default_value = "")]
    ca_cert: String,

    /// Output as JSON (for commands that produce structured output)
    #[arg(long, global = true)]
    json: bool,

    /// Path to .nixfleet.toml config file (default: walk up from cwd)
    #[arg(long, global = true)]
    config: Option<String>,

    /// Increase verbosity (-v for info, -vv for debug)
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy config to fleet hosts
    Deploy {
        /// Host patterns (glob-style, comma-separated, e.g. "web-*,db-01" or "*")
        #[arg(long, value_delimiter = ',', default_value = "*")]
        hosts: Vec<String>,

        /// Dry run: build closures and show what would happen, but don't push
        #[arg(long)]
        dry_run: bool,

        /// SSH fallback mode: copy closures and switch via SSH instead of control plane
        #[arg(long, help_heading = "SSH Mode")]
        ssh: bool,

        /// SSH target override (e.g. root@192.168.1.10). When set with --ssh,
        /// uses this address instead of resolving the hostname.
        /// Only valid with a single host (--hosts must match exactly one).
        #[arg(long, help_heading = "SSH Mode")]
        target: Option<String>,

        /// Flake reference (default: current directory)
        #[arg(long, default_value = ".")]
        flake: String,

        /// Target tags for rollout deploy (comma-separated or repeatable)
        #[arg(
            long,
            value_delimiter = ',',
            value_name = "TAG",
            help_heading = "Rollout"
        )]
        tags: Vec<String>,

        /// Rollout strategy: canary, staged, or all-at-once
        #[arg(long, default_value = "all-at-once", help_heading = "Rollout")]
        strategy: String,

        /// Batch sizes (comma-separated, e.g. "1,25%,100%")
        #[arg(long, value_delimiter = ',', help_heading = "Rollout")]
        batch_size: Option<Vec<String>>,

        /// Allow up to N unhealthy machines per batch (the (N+1)th fails the batch).
        /// 0 means zero tolerance — any single failure pauses the rollout.
        /// Accepts an absolute count (e.g. "3") or a percentage (e.g. "30%").
        #[arg(long, default_value = "0", help_heading = "Rollout")]
        failure_threshold: String,

        /// Action on failure: pause or revert
        #[arg(long, default_value = "pause", help_heading = "Rollout")]
        on_failure: String,

        /// Health check timeout in seconds
        #[arg(long, default_value = "300", help_heading = "Rollout")]
        health_timeout: u64,

        /// Wait and stream rollout progress
        #[arg(long, help_heading = "Rollout")]
        wait: bool,

        /// Release ID to deploy
        #[arg(long, help_heading = "Rollout")]
        release: Option<String>,

        /// Implicitly create a release and push to a Nix binary cache (s3://, ssh://, or HTTP URL)
        #[arg(long, conflicts_with = "release", help_heading = "Build & Push")]
        push_to: Option<String>,

        /// Use hook mode: push via push-cmd instead of nix copy
        #[arg(long, help_heading = "Hook Mode")]
        hook: bool,

        /// Override hook push command ({} = store path)
        #[arg(long, requires = "hook", help_heading = "Hook Mode")]
        hook_push_cmd: Option<String>,

        /// Override hook cache URL for agents to pull from
        #[arg(long, requires = "hook", help_heading = "Hook Mode")]
        hook_url: Option<String>,

        /// Implicitly create a release and copy closures via SSH
        #[arg(
            long,
            conflicts_with = "release",
            conflicts_with = "push_to",
            help_heading = "Build & Push"
        )]
        copy: bool,

        /// Binary cache URL for agents to fetch closures from (e.g. http://cache:5000)
        #[arg(long, help_heading = "Build & Push")]
        cache_url: Option<String>,
    },

    /// Show fleet status from the control plane
    Status {},

    /// Rollback a host to a previous generation
    Rollback {
        /// Target host name
        #[arg(long)]
        host: String,

        /// Store path of the generation to roll back to (default: previous)
        #[arg(long)]
        generation: Option<String>,

        /// SSH mode (always enabled, accepted for compatibility)
        #[arg(long, hide = true)]
        ssh: bool,

        /// SSH target override (e.g. root@192.168.1.10)
        #[arg(long)]
        target: Option<String>,
    },

    /// Manage fleet hosts
    Host {
        #[command(subcommand)]
        action: HostAction,
    },

    /// Manage rollouts
    Rollout {
        #[command(subcommand)]
        action: RolloutAction,
    },

    /// Manage machines and tags
    Machines {
        #[command(subcommand)]
        action: MachineAction,
    },

    /// Manage releases
    Release {
        #[command(subcommand)]
        action: ReleaseAction,
    },

    /// Bootstrap the first admin API key (only works when no keys exist)
    Bootstrap {
        /// Name for the admin key
        #[arg(long, default_value = "admin")]
        name: String,
    },

    /// Initialize a .nixfleet.toml config file
    Init {
        /// Control plane URL
        #[arg(long)]
        control_plane_url: String,
        /// CA certificate path
        #[arg(long)]
        ca_cert: Option<String>,
        /// Client certificate path (supports ${HOSTNAME} expansion)
        #[arg(long)]
        client_cert: Option<String>,
        /// Client key path (supports ${HOSTNAME} expansion)
        #[arg(long)]
        client_key: Option<String>,
        /// Default cache URL
        #[arg(long)]
        cache_url: Option<String>,
        /// Default push destination (nix copy --to)
        #[arg(long)]
        push_to: Option<String>,
        /// Cache URL when using --hook mode (e.g. http://cache:8081/mycache)
        #[arg(long)]
        hook_url: Option<String>,
        /// Push command for --hook mode ({} is replaced with store path)
        #[arg(long)]
        hook_push_cmd: Option<String>,
        /// Default deploy strategy (canary, staged, all-at-once)
        #[arg(long)]
        strategy: Option<String>,
        /// Default deploy failure action (pause, revert)
        #[arg(long)]
        on_failure: Option<String>,
    },
}

#[derive(Subcommand)]
enum HostAction {
    /// Scaffold a new host (generate configs, print fleet.nix snippet)
    Add {
        /// Host name for the new machine
        #[arg(long)]
        hostname: String,

        /// Organization name
        #[arg(long, default_value = "my-org")]
        org: String,

        /// Host role (workstation, server, edge, kiosk)
        #[arg(long, default_value = "workstation")]
        role: String,

        /// Target platform
        #[arg(long, default_value = "x86_64-linux")]
        platform: String,

        /// SSH target to fetch hardware config from (e.g. root@192.168.1.42)
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
enum RolloutAction {
    /// List rollouts
    List {
        /// Filter by status (e.g. running, paused, completed)
        #[arg(long)]
        status: Option<String>,
    },

    /// Show rollout detail with batch breakdown
    Status {
        /// Rollout ID
        id: String,
    },

    /// Resume a paused rollout
    Resume {
        /// Rollout ID
        id: String,
    },

    /// Cancel a rollout
    Cancel {
        /// Rollout ID
        id: String,
    },

    /// Delete a terminal rollout (completed, cancelled, or failed)
    Delete {
        /// Rollout ID
        id: String,
    },
}

#[derive(Subcommand)]
enum ReleaseAction {
    /// Build host closures, optionally distribute, and register a release
    Create {
        #[arg(long, default_value = ".")]
        flake: String,
        /// Host patterns (glob-style, comma-separated, e.g. "web-*,db-01" or "*")
        #[arg(long, value_delimiter = ',', default_value = "*")]
        hosts: Vec<String>,
        /// Push closures to a Nix binary cache (s3://, ssh://, or HTTP URL)
        #[arg(long)]
        push_to: Option<String>,
        /// Use hook mode: push via push-cmd instead of nix copy
        #[arg(long)]
        hook: bool,
        /// Override hook push command ({} = store path)
        #[arg(long, requires = "hook")]
        hook_push_cmd: Option<String>,
        /// Override hook cache URL for agents to pull from
        #[arg(long, requires = "hook")]
        hook_url: Option<String>,
        /// Copy closures to each host via nix-copy-closure
        #[arg(long, conflicts_with = "push_to")]
        copy: bool,
        /// Cache URL to record in release
        #[arg(long)]
        cache_url: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// Evaluate store paths without building (assumes closures in cache)
        #[arg(
            long,
            conflicts_with = "push_to",
            conflicts_with = "hook",
            conflicts_with = "copy"
        )]
        eval_only: bool,
    },
    /// List recent releases
    List {
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Filter by hostname
        #[arg(long)]
        host: Option<String>,
    },
    /// Show release details
    Show { release_id: String },
    /// Diff two releases
    Diff {
        release_id_a: String,
        release_id_b: String,
    },
    /// Delete a release (only if no rollout references it)
    Delete { release_id: String },
}

#[derive(Subcommand)]
enum MachineAction {
    /// List machines
    List {
        /// Filter by tags (comma-separated or repeatable)
        #[arg(long, value_delimiter = ',', value_name = "TAG")]
        tags: Vec<String>,
    },

    /// Change machine lifecycle state
    SetLifecycle {
        /// Machine ID
        id: String,
        /// Target state (active, pending, provisioning, maintenance, decommissioned)
        state: String,
    },

    /// Clear a machine's desired generation
    ClearDesired {
        /// Machine ID
        id: String,
    },

    /// Register a machine with the control plane
    Register {
        /// Machine ID
        id: String,

        /// Initial tags (comma-separated or repeatable)
        #[arg(long, value_delimiter = ',', value_name = "TAG")]
        tags: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    display::set_verbosity(cli.verbose);

    let default_level = match cli.verbose {
        0 => "nixfleet=warn",
        1 => "nixfleet=info",
        _ => "nixfleet=debug",
    };
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(display::SharedWriter::new()))
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_level.into()),
        )
        .init();

    // Load config file (--config flag or walk up from cwd)
    let cwd = std::env::current_dir().unwrap_or_default();
    let config_path = cli
        .config
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| config::find_config_file(&cwd));
    let (config_file, config_dir) = match &config_path {
        Some(path) => {
            let cfg = config::load_config_file(path).unwrap_or_else(|e| {
                tracing::warn!("failed to load {}: {}", path.display(), e);
                config::ConfigFile::default()
            });
            let dir = path.parent().unwrap_or(Path::new("."));
            (Some(cfg), Some(dir.to_path_buf()))
        }
        None => (None, None),
    };

    // Load credentials
    let credentials = config::load_credentials().unwrap_or_else(|e| {
        tracing::warn!("failed to load credentials: {}", e);
        config::CredentialsFile::default()
    });

    // Resolve config: file → credentials → env → CLI flags
    let resolved = config::resolve(
        config_file.as_ref(),
        config_dir.as_deref(),
        &credentials,
        config::CliOverrides {
            cp_url: &cli.control_plane_url,
            api_key: &cli.api_key,
            ca_cert: &cli.ca_cert,
            client_cert: &cli.client_cert,
            client_key: &cli.client_key,
        },
    );

    // Use resolved values for connection
    let effective_cp_url = resolved
        .control_plane_url
        .as_deref()
        .unwrap_or(&cli.control_plane_url);
    let effective_api_key = resolved.api_key.as_deref().unwrap_or(&cli.api_key);
    let effective_ca_cert = resolved.ca_cert.as_deref().unwrap_or(&cli.ca_cert);
    let effective_client_cert = resolved.client_cert.as_deref().unwrap_or(&cli.client_cert);
    let effective_client_key = resolved.client_key.as_deref().unwrap_or(&cli.client_key);

    // Warn if mTLS certs are set but URL is plaintext HTTP
    if !effective_client_cert.is_empty() && effective_cp_url.starts_with("http://") {
        eprintln!(
            "WARNING: --client-cert is set but control plane URL uses http:// (not https://). \
             Client certificates will not be sent over plaintext connections."
        );
    }

    let tls = client::TlsConfig {
        client_cert: effective_client_cert,
        client_key: effective_client_key,
        ca_cert: effective_ca_cert,
    };

    let json_output = cli.json;

    match cli.command {
        Commands::Deploy {
            hosts,
            dry_run,
            ssh,
            target,
            flake,
            tags,
            strategy,
            batch_size,
            failure_threshold,
            on_failure,
            health_timeout,
            wait,
            release,
            push_to,
            hook,
            hook_push_cmd,
            hook_url,
            copy,
            cache_url,
        } => {
            let http_client = client::build_client(&tls, effective_api_key)?;

            // --hook mode: use hook config for push-cmd and cache-url
            let (effective_push_to, effective_push_hook, effective_cache_url) = if hook {
                let push_cmd = hook_push_cmd.as_deref()
                    .or(resolved.hook_push_cmd.as_deref())
                    .ok_or_else(|| {
                        anyhow::anyhow!("--hook requires --hook-push-cmd or [cache.hook] push-cmd in .nixfleet.toml")
                    })?;
                let hook_cache = cache_url
                    .as_deref()
                    .or(hook_url.as_deref())
                    .or(resolved.hook_url.as_deref())
                    .or(resolved.cache_url.as_deref());
                (push_to.as_deref(), Some(push_cmd), hook_cache)
            } else {
                let pt = push_to.as_deref().or(resolved.push_to.as_deref());
                let cu = cache_url.as_deref().or(resolved.cache_url.as_deref());
                (pt, None, cu)
            };
            let effective_strategy = if strategy == "all-at-once" {
                // "all-at-once" is the clap default — check if config has a different default
                resolved.strategy.as_deref().unwrap_or(&strategy)
            } else {
                &strategy
            };

            if ssh {
                deploy::run(
                    &http_client,
                    effective_cp_url,
                    &hosts,
                    &flake,
                    dry_run,
                    true,
                    target.as_deref(),
                )
                .await
            } else {
                // Resolve release ID: explicit, or implicit via --push-to/--copy
                let release_id = if let Some(id) = release {
                    id
                } else if effective_push_to.is_some() || copy || effective_push_hook.is_some() {
                    let id = crate::release::create(
                        &http_client,
                        effective_cp_url,
                        &flake,
                        &hosts,
                        effective_push_to,
                        effective_push_hook,
                        copy,
                        effective_cache_url,
                        dry_run,
                        false,
                    )
                    .await?;
                    match id {
                        Some(id) => id,
                        None => return Ok(()), // dry-run, nothing to deploy
                    }
                } else {
                    bail!(
                        "--release, --push-to, --hook, or --copy is required for non-SSH deploys"
                    );
                };

                // --tags takes precedence; otherwise pass explicit host names
                let rollout_hosts: Vec<String> = if !tags.is_empty() {
                    vec![]
                } else {
                    hosts.iter().filter(|s| *s != "*").cloned().collect()
                };

                deploy::deploy_rollout(
                    &http_client,
                    effective_cp_url,
                    &release_id,
                    &tags,
                    &rollout_hosts,
                    effective_strategy,
                    batch_size,
                    &failure_threshold,
                    &on_failure,
                    health_timeout,
                    wait,
                    effective_cache_url,
                )
                .await
            }
        }
        Commands::Status {} => {
            let http_client = client::build_client(&tls, effective_api_key)?;
            status::run(&http_client, effective_cp_url, json_output).await
        }
        Commands::Rollback {
            host,
            generation,
            ssh: _,
            target,
        } => {
            let http_client = client::build_client(&tls, effective_api_key)?;
            rollback(
                &http_client,
                effective_cp_url,
                &host,
                generation,
                target.as_deref(),
            )
            .await
        }
        Commands::Host { action } => match action {
            HostAction::Add {
                hostname,
                org,
                role,
                platform,
                target,
            } => {
                host::add_host(
                    &hostname,
                    &org,
                    &role,
                    &platform,
                    target.as_deref(),
                    effective_cp_url,
                )
                .await
            }
        },
        Commands::Rollout { action } => {
            let http_client = client::build_client(&tls, effective_api_key)?;
            match action {
                RolloutAction::List { status } => {
                    rollout::list(
                        &http_client,
                        effective_cp_url,
                        status.as_deref(),
                        json_output,
                    )
                    .await
                }
                RolloutAction::Status { id } => {
                    rollout::status(&http_client, effective_cp_url, &id, json_output).await
                }
                RolloutAction::Resume { id } => {
                    rollout::resume(&http_client, effective_cp_url, &id).await
                }
                RolloutAction::Cancel { id } => {
                    rollout::cancel(&http_client, effective_cp_url, &id).await
                }
                RolloutAction::Delete { id } => {
                    rollout::delete(&http_client, effective_cp_url, &id).await
                }
            }
        }
        Commands::Release { action } => {
            let http_client = client::build_client(&tls, effective_api_key)?;
            match action {
                ReleaseAction::Create {
                    flake,
                    hosts,
                    push_to,
                    hook,
                    hook_push_cmd,
                    hook_url,
                    copy,
                    cache_url,
                    dry_run,
                    eval_only,
                } => {
                    let (effective_push_to, effective_push_hook, effective_cache_url) = if hook {
                        let push_cmd = hook_push_cmd.as_deref()
                            .or(resolved.hook_push_cmd.as_deref())
                            .ok_or_else(|| {
                                anyhow::anyhow!("--hook requires --hook-push-cmd or [cache.hook] push-cmd in .nixfleet.toml")
                            })?;
                        let hook_cache = cache_url
                            .as_deref()
                            .or(hook_url.as_deref())
                            .or(resolved.hook_url.as_deref())
                            .or(resolved.cache_url.as_deref());
                        (
                            push_to.as_deref().map(str::to_string),
                            Some(push_cmd),
                            hook_cache.map(str::to_string),
                        )
                    } else {
                        let pt = push_to.or_else(|| resolved.push_to.clone());
                        let cu = cache_url.or_else(|| resolved.cache_url.clone());
                        (pt, None, cu)
                    };
                    release::create(
                        &http_client,
                        effective_cp_url,
                        &flake,
                        &hosts,
                        effective_push_to.as_deref(),
                        effective_push_hook,
                        copy,
                        effective_cache_url.as_deref(),
                        dry_run,
                        eval_only,
                    )
                    .await?;
                    Ok(())
                }
                ReleaseAction::List { limit, host } => {
                    release::list(
                        &http_client,
                        effective_cp_url,
                        limit,
                        host.as_deref(),
                        json_output,
                    )
                    .await
                }
                ReleaseAction::Show { release_id } => {
                    release::show(&http_client, effective_cp_url, &release_id, json_output).await
                }
                ReleaseAction::Diff {
                    release_id_a,
                    release_id_b,
                } => {
                    release::diff(
                        &http_client,
                        effective_cp_url,
                        &release_id_a,
                        &release_id_b,
                        json_output,
                    )
                    .await
                }
                ReleaseAction::Delete { release_id } => {
                    release::delete(&http_client, effective_cp_url, &release_id).await
                }
            }
        }
        Commands::Machines { action } => {
            let http_client = client::build_client(&tls, effective_api_key)?;
            match action {
                MachineAction::List { tags } => {
                    machines::list(&http_client, effective_cp_url, &tags, json_output).await
                }
                MachineAction::SetLifecycle { id, state } => {
                    machines::set_lifecycle(&http_client, effective_cp_url, &id, &state).await
                }
                MachineAction::ClearDesired { id } => {
                    machines::clear_desired(&http_client, effective_cp_url, &id).await
                }
                MachineAction::Register { id, tags } => {
                    machines::register(&http_client, effective_cp_url, &id, &tags).await
                }
            }
        }
        Commands::Bootstrap { name } => {
            // Bootstrap does not require an API key, but does use mTLS
            let http_client = client::build_client(&tls, "")?;
            let result = bootstrap(&http_client, effective_cp_url, &name, json_output).await;
            // Save API key to credentials file
            if let Ok(Some(ref key_str)) = result {
                if let Err(e) = config::save_api_key(effective_cp_url, key_str) {
                    eprintln!("Warning: failed to save API key: {}", e);
                } else {
                    println!("Saved to {}", config::credentials_path().display());
                }
            }
            result.map(|_| ())
        }
        Commands::Init {
            control_plane_url,
            ca_cert,
            client_cert,
            client_key,
            cache_url,
            push_to,
            hook_url,
            hook_push_cmd,
            strategy,
            on_failure,
        } => {
            let path = cwd.join(".nixfleet.toml");
            config::write_config_file(
                &path,
                &control_plane_url,
                ca_cert.as_deref(),
                client_cert.as_deref(),
                client_key.as_deref(),
                cache_url.as_deref(),
                push_to.as_deref(),
                hook_url.as_deref(),
                hook_push_cmd.as_deref(),
                strategy.as_deref(),
                on_failure.as_deref(),
            )?;
            println!("Config written to {}", path.display());
            Ok(())
        }
    }
}

async fn rollback(
    _client: &reqwest::Client,
    _cp_url: &str,
    host: &str,
    generation: Option<String>,
    target: Option<&str>,
) -> Result<()> {
    let default_dest = format!("root@{}", host);
    let ssh_dest = target.unwrap_or(&default_dest);

    let store_path = match generation {
        Some(path) => path,
        None => {
            // Resolve previous generation: list profile links, sort
            // numerically, take the second-to-last. More robust than
            // parsing the current profile number with sed.
            let output = tokio::process::Command::new("ssh")
                .args([
                    ssh_dest,
                    "ls -dv /nix/var/nix/profiles/system-*-link | tail -2 | head -1 | xargs readlink -f",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .context("Failed to query previous generation via SSH")?;

            if !output.status.success() {
                bail!(
                    "Failed to get previous generation: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            String::from_utf8(output.stdout)?.trim().to_string()
        }
    };

    println!("Rolling back {} to {}", host, store_path);

    // SSH rollback: switch to the specified profile on the target
    let stderr = if display::passthrough_output() {
        Stdio::inherit()
    } else {
        Stdio::piped()
    };
    let switch_output = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            ssh_dest,
            &format!("{}/bin/switch-to-configuration", store_path),
            "switch",
        ])
        .stdout(Stdio::inherit())
        .stderr(stderr)
        .output()
        .await
        .context("SSH switch-to-configuration failed")?;

    if !switch_output.status.success() {
        let stderr = String::from_utf8_lossy(&switch_output.stderr);
        bail!("Rollback failed on {}: {}", host, stderr);
    }
    println!("Rollback complete on {}", host);

    Ok(())
}

/// Bootstrap returns Ok(Some(key)) on success, Ok(None) if JSON output mode (key printed inline).
async fn bootstrap(
    client: &reqwest::Client,
    cp_url: &str,
    name: &str,
    json_output: bool,
) -> Result<Option<String>> {
    let url = format!("{}/api/v1/keys/bootstrap", cp_url);
    let body = serde_json::json!({ "name": name });

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("failed to reach control plane")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = client::read_error_body(resp).await;
        if status.as_u16() == 409 {
            bail!("Bootstrap failed: API keys already exist. Use an existing admin key to create new keys.");
        }
        bail!("Control plane returned {}: {}", status, body);
    }

    let payload: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse bootstrap response")?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload)
                .context("failed to serialize bootstrap response")?
        );
        // Extract key for auto-save even in JSON mode
        let key = payload["key"].as_str().map(|s| s.to_string());
        Ok(key)
    } else {
        let key = payload["key"]
            .as_str()
            .context("response missing 'key' field")?;
        let role = payload["role"].as_str().unwrap_or("admin");
        eprintln!("API key created (name: {}, role: {})", name, role);
        println!("{}", key);
        Ok(Some(key.to_string()))
    }
}

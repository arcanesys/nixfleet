use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::process::Stdio;

mod client;
mod deploy;
mod host;
mod machines;
mod rollout;
mod status;

#[derive(Parser)]
#[command(name = "nixfleet", about = "NixFleet fleet management CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Control plane URL
    #[arg(
        long,
        global = true,
        default_value = "http://localhost:8080",
        env = "NIXFLEET_CP_URL"
    )]
    control_plane_url: String,

    /// API key for control plane authentication
    #[arg(long, global = true, default_value = "", env = "NIXFLEET_API_KEY")]
    api_key: String,

    /// Client certificate for mTLS authentication
    #[arg(long, global = true, default_value = "", env = "NIXFLEET_CLIENT_CERT")]
    client_cert: String,

    /// Client key for mTLS authentication
    #[arg(long, global = true, default_value = "", env = "NIXFLEET_CLIENT_KEY")]
    client_key: String,

    /// CA certificate for TLS verification (optional, uses system trust store if omitted)
    #[arg(long, global = true, default_value = "", env = "NIXFLEET_CA_CERT")]
    ca_cert: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy config to fleet hosts
    Deploy {
        /// Host pattern (glob-style, e.g. "web*" or "*") — used with --ssh mode
        #[arg(long, default_value = "*")]
        hosts: String,

        /// Dry run: build closures and show what would happen, but don't push
        #[arg(long)]
        dry_run: bool,

        /// SSH fallback mode: copy closures and switch via SSH instead of control plane
        #[arg(long)]
        ssh: bool,

        /// Flake reference (default: current directory)
        #[arg(long, default_value = ".")]
        flake: String,

        /// Target tag for rollout deploy (repeatable)
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,

        /// Rollout strategy: canary, staged, or all-at-once
        #[arg(long, default_value = "all-at-once")]
        strategy: String,

        /// Batch sizes (comma-separated, e.g. "1,25%,100%")
        #[arg(long, value_delimiter = ',')]
        batch_size: Option<Vec<String>>,

        /// Maximum failures before pausing/reverting
        #[arg(long, default_value = "1")]
        failure_threshold: String,

        /// Action on failure: pause or revert
        #[arg(long, default_value = "pause")]
        on_failure: String,

        /// Health check timeout in seconds
        #[arg(long, default_value = "300")]
        health_timeout: u64,

        /// Wait and stream rollout progress
        #[arg(long)]
        wait: bool,

        /// Store path hash for rollout deploy (skips nix build)
        #[arg(long)]
        generation: Option<String>,
    },

    /// Show fleet status from the control plane
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Rollback a host to a previous generation
    Rollback {
        /// Target host name
        #[arg(long)]
        host: String,

        /// Store path of the generation to roll back to (default: previous)
        #[arg(long)]
        generation: Option<String>,

        /// SSH fallback mode
        #[arg(long)]
        ssh: bool,
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

    /// Bootstrap the first admin API key (only works when no keys exist)
    Bootstrap {
        /// Name for the admin key
        #[arg(long, default_value = "admin")]
        name: String,

        /// Output raw JSON instead of human-friendly format
        #[arg(long)]
        json: bool,
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

    /// Provision a host (install NixOS via nixos-anywhere)
    Provision {
        /// Host name (must exist in flake)
        #[arg(long)]
        hostname: String,

        /// SSH target (e.g. root@192.168.1.42)
        #[arg(long)]
        target: String,

        /// Username for post-install verification
        #[arg(long, default_value = "root")]
        username: String,
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
}

#[derive(Subcommand)]
enum MachineAction {
    /// List machines
    List {
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
    },

    /// Add tags to a machine
    Tag {
        /// Machine ID
        id: String,

        /// Tags to add
        #[arg(required = true)]
        tags: Vec<String>,
    },

    /// Remove a tag from a machine
    Untag {
        /// Machine ID
        id: String,

        /// Tag to remove
        tag: String,
    },

    /// Register a machine with the control plane
    Register {
        /// Machine ID
        id: String,

        /// Initial tags
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixfleet=info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Warn if mTLS certs are set but URL is plaintext HTTP
    if !cli.client_cert.is_empty() && cli.control_plane_url.starts_with("http://") {
        eprintln!(
            "WARNING: --client-cert is set but control plane URL uses http:// (not https://). \
             Client certificates will not be sent over plaintext connections."
        );
    }

    let tls = client::TlsConfig {
        client_cert: &cli.client_cert,
        client_key: &cli.client_key,
        ca_cert: &cli.ca_cert,
    };

    match cli.command {
        Commands::Deploy {
            hosts,
            dry_run,
            ssh,
            flake,
            tags,
            strategy,
            batch_size,
            failure_threshold,
            on_failure,
            health_timeout,
            wait,
            generation,
        } => {
            let http_client = client::build_client(&tls, &cli.api_key)?;
            if ssh {
                deploy::run(
                    &http_client,
                    &cli.control_plane_url,
                    &hosts,
                    &flake,
                    dry_run,
                    true,
                )
                .await
            } else if !tags.is_empty() || generation.is_some() {
                // Rollout deploy mode
                let generation_hash = match generation {
                    Some(hash) => hash,
                    None => {
                        bail!(
                            "--generation is required for rollout deploy (store path of the built closure)"
                        );
                    }
                };
                deploy::deploy_rollout(
                    &http_client,
                    &cli.control_plane_url,
                    &cli.api_key,
                    &generation_hash,
                    &tags,
                    &[],
                    &strategy,
                    batch_size,
                    &failure_threshold,
                    &on_failure,
                    health_timeout,
                    wait,
                )
                .await
            } else {
                deploy::run(
                    &http_client,
                    &cli.control_plane_url,
                    &hosts,
                    &flake,
                    dry_run,
                    false,
                )
                .await
            }
        }
        Commands::Status { json } => {
            let http_client = client::build_client(&tls, &cli.api_key)?;
            status::run(&http_client, &cli.control_plane_url, json).await
        }
        Commands::Rollback {
            host,
            generation,
            ssh,
        } => {
            let http_client = client::build_client(&tls, &cli.api_key)?;
            rollback(&http_client, &cli.control_plane_url, &host, generation, ssh).await
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
                    &cli.control_plane_url,
                )
                .await
            }
            HostAction::Provision {
                hostname,
                target,
                username,
            } => host::provision_host(&hostname, &target, &username).await,
        },
        Commands::Rollout { action } => {
            let http_client = client::build_client(&tls, &cli.api_key)?;
            match action {
                RolloutAction::List { status } => {
                    rollout::list(&http_client, &cli.control_plane_url, status.as_deref()).await
                }
                RolloutAction::Status { id } => {
                    rollout::status(&http_client, &cli.control_plane_url, &id).await
                }
                RolloutAction::Resume { id } => {
                    rollout::resume(&http_client, &cli.control_plane_url, &id).await
                }
                RolloutAction::Cancel { id } => {
                    rollout::cancel(&http_client, &cli.control_plane_url, &id).await
                }
            }
        }
        Commands::Machines { action } => {
            let http_client = client::build_client(&tls, &cli.api_key)?;
            match action {
                MachineAction::List { tag } => {
                    machines::list(&http_client, &cli.control_plane_url, tag.as_deref()).await
                }
                MachineAction::Tag { id, tags } => {
                    machines::tag(&http_client, &cli.control_plane_url, &id, &tags).await
                }
                MachineAction::Untag { id, tag } => {
                    machines::untag(&http_client, &cli.control_plane_url, &id, &tag).await
                }
                MachineAction::Register { id, tags } => {
                    machines::register(&http_client, &cli.control_plane_url, &id, &tags).await
                }
            }
        }
        Commands::Bootstrap { name, json } => {
            // Bootstrap does not require an API key, but does use mTLS
            let http_client = client::build_client(&tls, "")?;
            bootstrap(&http_client, &cli.control_plane_url, &name, json).await
        }
    }
}

async fn rollback(
    client: &reqwest::Client,
    cp_url: &str,
    host: &str,
    generation: Option<String>,
    ssh: bool,
) -> Result<()> {
    let store_path = match generation {
        Some(path) => path,
        None => {
            if ssh {
                // Get previous generation via SSH
                let output = tokio::process::Command::new("ssh")
                    .args([
                        &format!("root@{}", host),
                        "readlink",
                        "-f",
                        "/nix/var/nix/profiles/system-1-link",
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
            } else {
                bail!("--generation is required when not using --ssh mode (control plane does not track generation history yet)");
            }
        }
    };

    println!("Rolling back {} to {}", host, store_path);

    if ssh {
        // SSH rollback: switch to the specified profile
        let switch_output = tokio::process::Command::new("ssh")
            .args([
                &format!("root@{}", host),
                &format!("{}/bin/switch-to-configuration", store_path),
                "switch",
            ])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("SSH switch-to-configuration failed")?;

        if !switch_output.success() {
            bail!("Rollback failed on {}", host);
        }
        println!("Rollback complete on {}", host);
    } else {
        // Control plane mode: set the desired generation to the rollback target
        let url = format!("{}/api/v1/machines/{}/set-generation", cp_url, host);

        let resp = client
            .post(&url)
            .json(&serde_json::json!({ "hash": store_path }))
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
        println!(
            "Desired generation set to {} for {} (agent will pick up on next poll)",
            store_path, host
        );
    }

    Ok(())
}

async fn bootstrap(
    client: &reqwest::Client,
    cp_url: &str,
    name: &str,
    json_output: bool,
) -> Result<()> {
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
        let body = resp.text().await.unwrap_or_default();
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
    } else {
        let key = payload["key"]
            .as_str()
            .context("response missing 'key' field")?;
        let role = payload["role"].as_str().unwrap_or("admin");
        eprintln!("API key created (name: {}, role: {})", name, role);
        println!("{}", key);
    }

    Ok(())
}

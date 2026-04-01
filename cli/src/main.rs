use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::process::Stdio;

mod deploy;
mod host;
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
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy config to fleet hosts
    Deploy {
        /// Host pattern (glob-style, e.g. "web*" or "*")
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
}

#[derive(Subcommand)]
enum HostAction {
    /// Scaffold a new host (generate configs, print fleet.nix snippet)
    Add {
        /// Host name for the new machine
        #[arg(long)]
        hostname: String,

        /// Organization name
        #[arg(long, default_value = "abstracts33d")]
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixfleet=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Deploy {
            hosts,
            dry_run,
            ssh,
            flake,
        } => deploy::run(&cli.control_plane_url, &hosts, &flake, dry_run, ssh).await,
        Commands::Status { json } => status::run(&cli.control_plane_url, json).await,
        Commands::Rollback {
            host,
            generation,
            ssh,
        } => rollback(&cli.control_plane_url, &host, generation, ssh).await,
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
    }
}

async fn rollback(cp_url: &str, host: &str, generation: Option<String>, ssh: bool) -> Result<()> {
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
        let client = reqwest::Client::new();
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

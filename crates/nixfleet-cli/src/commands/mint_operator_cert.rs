//! Operator-side helper: mints a clientAuth-EKU X.509 cert from the offline
//! fleet root CA. Pure offline crypto, no network access.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
// Alias avoids clashing with `struct Args` below.
use clap::Args as ClapArgs;

use crate::{MintOperatorCertArgs, mint_operator_cert};

#[derive(ClapArgs, Debug)]
#[command(
    about = "Mint an mTLS client cert for an operator workstation, signed by the offline fleet root CA."
)]
pub struct Args {
    /// Offline fleet root CA cert PEM. Falls back to
    /// $NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE then to
    /// ~/.config/nixfleet/fleet-root.cert.pem.
    #[arg(long)]
    root_cert: Option<PathBuf>,

    /// Offline fleet root CA private key PEM. Falls back to
    /// $NIXFLEET_OPERATOR_FLEET_ROOT_KEY_FILE then to
    /// ~/.config/nixfleet/fleet-root.key.pem.
    #[arg(long)]
    root_key: Option<PathBuf>,

    /// Common Name on the operator cert. Default: operator-${USER}@${HOSTNAME}.
    #[arg(long)]
    cn: Option<String>,

    /// Output cert path. Default: ~/.config/nixfleet/operator.pem.
    #[arg(long)]
    output_cert: Option<PathBuf>,

    /// Output key path. Default: ~/.config/nixfleet/operator.key.
    #[arg(long)]
    output_key: Option<PathBuf>,

    /// Validity in days.
    #[arg(long, default_value_t = 365)]
    days: u32,

    /// Overwrite existing operator.pem / operator.key.
    #[arg(long)]
    force: bool,
}

pub fn run(args: Args) -> Result<()> {
    let cfg_dir = crate::config::default_config_path()
        .parent()
        .map(|p| p.to_path_buf())
        .context("resolve ~/.config/nixfleet directory")?;

    let root_cert = args
        .root_cert
        .or_else(|| std::env::var_os("NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE").map(PathBuf::from))
        .unwrap_or_else(|| cfg_dir.join("fleet-root.cert.pem"));
    let root_key = args
        .root_key
        .or_else(|| std::env::var_os("NIXFLEET_OPERATOR_FLEET_ROOT_KEY_FILE").map(PathBuf::from))
        .unwrap_or_else(|| cfg_dir.join("fleet-root.key.pem"));
    let output_cert = args
        .output_cert
        .unwrap_or_else(|| cfg_dir.join("operator.pem"));
    let output_key = args
        .output_key
        .unwrap_or_else(|| cfg_dir.join("operator.key"));

    let cn = match args.cn {
        Some(c) => c,
        None => {
            let user = std::env::var("USER").unwrap_or_default();
            let host = whoami::fallible::hostname().unwrap_or_default();
            if user.is_empty() || host.is_empty() {
                bail!("operator CN is empty (USER={user:?}, HOSTNAME={host:?}); pass --cn");
            }
            format!("operator-{user}@{host}")
        }
    };

    let outcome = mint_operator_cert(MintOperatorCertArgs {
        root_cert_path: root_cert,
        root_key_path: root_key,
        cn,
        output_cert_path: output_cert,
        output_key_path: output_key,
        validity_days: args.days,
        overwrite: args.force,
    })?;

    eprintln!(
        "minted operator cert
  cn:          {}
  valid until: {} ({} days)
  cert:        {}
  key:         {}

next: nixfleet config init --client-cert {} --client-key {}",
        outcome.cn,
        outcome.not_after.to_rfc3339(),
        args.days,
        outcome.cert_path.display(),
        outcome.key_path.display(),
        outcome.cert_path.display(),
        outcome.key_path.display(),
    );
    Ok(())
}

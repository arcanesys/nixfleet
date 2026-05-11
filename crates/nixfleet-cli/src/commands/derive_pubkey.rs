//! Operator helper: ed25519 private key file → base64 public key.
//!
//! Folded from the former `nixfleet-derive-pubkey` binary. Subcommand
//! form: `nixfleet derive-pubkey <path>`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
// Alias required: `struct Args` below shares its name with the clap trait.
use clap::Args as ClapArgs;
use ed25519_dalek::{SigningKey, VerifyingKey};

#[derive(ClapArgs, Debug)]
#[command(about = "Derive base64 ed25519 pubkey from a raw private key file.")]
pub struct Args {
    /// Path to a private key file: 32 raw bytes, hex, or PKCS#8 PEM.
    pub private_key_path: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let bytes = std::fs::read(&args.private_key_path)
        .with_context(|| format!("read {}", args.private_key_path.display()))?;

    let arr: [u8; 32] = if bytes.len() >= 32 {
        bytes[..32]
            .try_into()
            .expect("slice of length 32 fits [u8; 32] — len checked above")
    } else {
        anyhow::bail!("expected at least 32 bytes, got {}", bytes.len());
    };
    let sk = SigningKey::from_bytes(&arr);
    let vk: VerifyingKey = sk.verifying_key();
    println!(
        "{}",
        base64::engine::general_purpose::STANDARD.encode(vk.to_bytes())
    );
    Ok(())
}

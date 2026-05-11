//! Sign-hook + smoke-verify + atomic release-dir write.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use nixfleet_proto::FleetResolved;
use tempfile::NamedTempFile;

use crate::canonicalize_resolved;

pub(crate) fn sign(cmd: &str, canonical: &[u8]) -> Result<Vec<u8>> {
    let input = NamedTempFile::new().context("create tempfile for canonical bytes")?;
    let output = NamedTempFile::new().context("create tempfile for signature")?;

    std::fs::write(input.path(), canonical).context("write canonical bytes to tempfile")?;
    // Pre-create empty so the hook only needs to overwrite, not also create.
    std::fs::write(output.path(), b"").ok();

    tracing::info!("sign hook");
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("NIXFLEET_INPUT", input.path())
        .env("NIXFLEET_OUTPUT", output.path())
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("invoke sign hook ({cmd:?})"))?;
    if !status.success() {
        bail!(
            "sign hook exited {} ({:?})",
            status.code().unwrap_or(-1),
            cmd,
        );
    }

    let sig = std::fs::read(output.path()).context("read signature output")?;
    if sig.is_empty() {
        bail!("sign hook produced 0-byte signature - refusing to publish");
    }
    Ok(sig)
}

/// Structural smoke verify: byte-stable canonicalize round-trip + schema
/// parse + non-zero sig. No cryptographic verification.
pub(crate) fn smoke_verify(canonical: &[u8], signature: &[u8]) -> Result<()> {
    let parsed: FleetResolved = serde_json::from_slice(canonical)
        .context("smoke verify: canonical bytes don't parse as FleetResolved")?;
    let recanonical =
        canonicalize_resolved(&parsed).context("smoke verify: re-canonicalize failed")?;
    if recanonical.as_bytes() != canonical {
        bail!("smoke verify: canonicalization is not byte-stable round-trip");
    }
    if signature.is_empty() {
        bail!("smoke verify: empty signature");
    }
    tracing::info!(sig_len = signature.len(), "smoke verify ok");
    Ok(())
}

pub(crate) fn write_release(
    release_dir: &Path,
    artifact_name: &str,
    canonical: &[u8],
    signature: &[u8],
) -> Result<()> {
    std::fs::create_dir_all(release_dir)
        .with_context(|| format!("create release dir {}", release_dir.display()))?;
    let artifact_path = release_dir.join(artifact_name);
    let signature_path = release_dir.join(format!("{artifact_name}.sig"));
    atomic_write(&artifact_path, canonical)?;
    atomic_write(&signature_path, signature)?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("tempfile in {}", dir.display()))?;
    use std::io::Write;
    tmp.write_all(bytes).context("write release tempfile")?;
    tmp.persist(path)
        .with_context(|| format!("rename tempfile to {}", path.display()))?;
    Ok(())
}

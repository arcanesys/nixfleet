use anyhow::{Context, Result};

/// Validate that a string looks like a `/nix/store/<hash>-<name>` path.
///
/// Defense-in-depth: store paths are interpolated into SSH commands.
/// Even though they come from `nix build` output, validate before
/// passing to remote shells.
pub fn store_path(store_path: &str) -> Result<()> {
    let rest = store_path
        .strip_prefix("/nix/store/")
        .with_context(|| format!("store path must start with /nix/store/: {store_path}"))?;
    if rest.is_empty() || rest.contains('/') || rest.contains("..") {
        anyhow::bail!("invalid store path: {store_path}");
    }
    let bytes = rest.as_bytes();
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
    {
        anyhow::bail!("invalid characters in store path: {store_path}");
    }
    Ok(())
}

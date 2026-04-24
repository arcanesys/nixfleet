//! JCS canonicalization library backing the `nixfleet-canonicalize`
//! binary. Pinned to `serde_jcs` per `docs/CONTRACTS.md §III`.
//!
//! Every signer and verifier in the fleet goes through this one
//! function — do not reimplement in Nix, shell, or ad-hoc Rust.

use anyhow::{Context, Result};

/// Canonicalize an arbitrary JSON string to JCS (RFC 8785) form.
///
/// Errors on malformed JSON. The returned string is the exact byte
/// sequence every signer must feed to its signature primitive and
/// every verifier must reconstruct before verification.
pub fn canonicalize(input: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(input).context("input is not valid JSON")?;
    serde_jcs::to_string(&value).context("JCS canonicalization failed")
}

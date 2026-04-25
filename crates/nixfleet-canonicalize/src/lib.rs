#![allow(clippy::doc_lazy_continuation)]
//! JCS canonicalization (RFC 8785). LOADBEARING: every signer and verifier
//! routes through here - do not reimplement, drift invalidates signatures
//! fleet-wide.

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::Digest;

/// JSON string -> JCS (RFC 8785) canonical form.
pub fn canonicalize(input: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(input).context("input is not valid JSON")?;
    serde_jcs::to_string(&value).context("JCS canonicalization failed")
}

/// Hex-lowercase SHA-256 of `value`'s JCS-canonical bytes.
pub fn sha256_jcs_hex<T: Serialize>(value: &T) -> Result<String> {
    let canonical = serde_jcs::to_vec(value).context("JCS canonicalization failed")?;
    let digest = sha2::Sha256::digest(&canonical);
    Ok(hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_jcs_hex_string_value_is_stable() {
        let a = sha256_jcs_hex(&"hello").unwrap();
        let b = sha256_jcs_hex(&"hello").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn sha256_jcs_hex_struct_value_is_stable() {
        #[derive(Serialize)]
        struct S<'a> {
            host: &'a str,
            count: u32,
        }
        let a = sha256_jcs_hex(&S {
            host: "host-02",
            count: 7,
        })
        .unwrap();
        let b = sha256_jcs_hex(&S {
            host: "host-02",
            count: 7,
        })
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn sha256_jcs_hex_empty_string_is_distinct_from_other_input() {
        let empty = sha256_jcs_hex(&"").unwrap();
        let nonempty = sha256_jcs_hex(&"x").unwrap();
        assert_ne!(empty, nonempty);
        assert_eq!(empty.len(), 64);
    }
}

//! Host SSH key primitives shared by agent enrollment, CP enroll/renew, and
//! mint_token. Kept pure-rust (no `ssh-key` dep) so the boundary-contract crate
//! stays lean - the canonical bridge from "OpenSSH host key bytes on disk" to
//! "rcgen-usable keypair" and bootstrap-token fingerprints.

use base64::Engine;
use sha2::{Digest, Sha256};

/// Parse a 32-byte ed25519 raw public key from an OpenSSH-format pubkey
/// line (`"ssh-ed25519 <base64> [comment]"`). Errors when the line isn't
/// ed25519, the base64 doesn't decode, or the inner SSH-wire-format
/// (RFC 4253 §6.6: `string "ssh-ed25519" || string <32-byte pubkey>`)
/// is malformed.
pub fn ed25519_pubkey_raw_from_openssh(line: &str) -> Result<[u8; 32], OpenSshParseError> {
    let trimmed = line.trim();
    let mut parts = trimmed.split_whitespace();
    let algo = parts.next().ok_or(OpenSshParseError::Empty)?;
    if algo != "ssh-ed25519" {
        return Err(OpenSshParseError::WrongAlgorithm {
            got: algo.to_string(),
        });
    }
    let blob_b64 = parts.next().ok_or(OpenSshParseError::MissingBase64)?;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(blob_b64)
        .map_err(|_| OpenSshParseError::InvalidBase64)?;

    // RFC 4253 §6.6 wire format: u32 big-endian length + bytes per field.
    let mut cursor = 0usize;
    let algo_bytes = read_ssh_string(&blob, &mut cursor)?;
    if algo_bytes != b"ssh-ed25519" {
        return Err(OpenSshParseError::InnerAlgorithmMismatch);
    }
    let pubkey = read_ssh_string(&blob, &mut cursor)?;
    if pubkey.len() != 32 {
        return Err(OpenSshParseError::WrongPubkeyLength { got: pubkey.len() });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(pubkey);
    Ok(out)
}

/// Wrap a 32-byte ed25519 seed in PKCS#8 v1 DER (RFC 8410, OID 1.3.101.112).
/// The 16-byte prefix is the fixed PKCS#8 envelope for an ed25519 seed:
///   `SEQUENCE(46) { INTEGER(0); SEQUENCE { OID 1.3.101.112 }; OCTET STRING(34) { OCTET STRING(32) <seed> } }`.
pub fn ed25519_pkcs8_der_from_seed(seed: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.extend_from_slice(&[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ]);
    out.extend_from_slice(seed);
    out
}

/// PEM-armoured form of [`ed25519_pkcs8_der_from_seed`], for callers
/// that need to hand a string to `rcgen::KeyPair::from_pem`.
pub fn ed25519_pkcs8_pem_from_seed(seed: &[u8; 32]) -> String {
    let der = ed25519_pkcs8_der_from_seed(seed);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&der);
    format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n")
}

/// Extract the 32-byte raw ed25519 pubkey from a SubjectPublicKeyInfo DER
/// blob (RFC 8410, what `rcgen::PublicKeyData::der_bytes()` returns). Validates
/// the fixed 12-byte SPKI prefix so callers reject non-ed25519 CSRs cleanly
/// instead of silently slicing.
pub fn ed25519_pubkey_raw_from_spki_der(spki: &[u8]) -> Result<[u8; 32], OpenSshParseError> {
    const ED25519_SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    if spki.len() != 44 {
        return Err(OpenSshParseError::WrongPubkeyLength { got: spki.len() });
    }
    if spki[..12] != ED25519_SPKI_PREFIX {
        return Err(OpenSshParseError::WrongAlgorithm {
            got: format!(
                "non-ed25519 SPKI (prefix={:x?})",
                &spki[..12.min(spki.len())]
            ),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&spki[12..]);
    Ok(out)
}

/// `base64(SHA-256(raw_pubkey))` - same shape as the bootstrap-token's
/// `expected_pubkey_fingerprint` field. Accepts an OpenSSH pubkey line
/// directly so callers don't double-parse.
pub fn fingerprint_openssh_pubkey(line: &str) -> Result<String, OpenSshParseError> {
    let raw = ed25519_pubkey_raw_from_openssh(line)?;
    let digest = Sha256::digest(raw);
    Ok(base64::engine::general_purpose::STANDARD.encode(digest))
}

fn read_ssh_string<'a>(buf: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], OpenSshParseError> {
    if *cursor + 4 > buf.len() {
        return Err(OpenSshParseError::Truncated);
    }
    let len = u32::from_be_bytes([
        buf[*cursor],
        buf[*cursor + 1],
        buf[*cursor + 2],
        buf[*cursor + 3],
    ]) as usize;
    *cursor += 4;
    if *cursor + len > buf.len() {
        return Err(OpenSshParseError::Truncated);
    }
    let s = &buf[*cursor..*cursor + len];
    *cursor += len;
    Ok(s)
}

#[derive(Debug)]
pub enum OpenSshParseError {
    Empty,
    WrongAlgorithm { got: String },
    MissingBase64,
    InvalidBase64,
    Truncated,
    InnerAlgorithmMismatch,
    WrongPubkeyLength { got: usize },
}

impl std::fmt::Display for OpenSshParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty OpenSSH pubkey line"),
            Self::WrongAlgorithm { got } => {
                write!(f, "expected ssh-ed25519 algorithm, got {got}")
            }
            Self::MissingBase64 => write!(f, "OpenSSH pubkey missing base64 blob"),
            Self::InvalidBase64 => write!(f, "OpenSSH pubkey base64 invalid"),
            Self::Truncated => write!(f, "OpenSSH wire blob truncated"),
            Self::InnerAlgorithmMismatch => {
                write!(f, "OpenSSH wire blob inner algo != ssh-ed25519")
            }
            Self::WrongPubkeyLength { got } => {
                write!(f, "ed25519 pubkey is not 32 bytes (got {got})")
            }
        }
    }
}

impl std::error::Error for OpenSshParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built fixture: ssh-ed25519 pubkey with all-0x42 raw bytes.
    fn make_openssh_line(raw: &[u8; 32]) -> String {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        blob.extend_from_slice(raw);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        format!("ssh-ed25519 {b64} test@host")
    }

    #[test]
    fn round_trips_round_pubkey_bytes() {
        let raw = [0x42u8; 32];
        let line = make_openssh_line(&raw);
        let got = ed25519_pubkey_raw_from_openssh(&line).expect("parse");
        assert_eq!(got, raw);
    }

    #[test]
    fn rejects_non_ed25519_algorithm() {
        let err = ed25519_pubkey_raw_from_openssh("ssh-rsa AAAA test").unwrap_err();
        matches!(err, OpenSshParseError::WrongAlgorithm { .. });
    }

    #[test]
    fn rejects_missing_blob() {
        let err = ed25519_pubkey_raw_from_openssh("ssh-ed25519").unwrap_err();
        matches!(err, OpenSshParseError::MissingBase64);
    }

    #[test]
    fn rejects_invalid_base64() {
        let err = ed25519_pubkey_raw_from_openssh("ssh-ed25519 !!!notbase64!!!").unwrap_err();
        matches!(err, OpenSshParseError::InvalidBase64);
    }

    #[test]
    fn pkcs8_envelope_has_expected_layout() {
        let seed = [0x55u8; 32];
        let der = ed25519_pkcs8_der_from_seed(&seed);
        assert_eq!(der.len(), 48);
        assert_eq!(
            &der[..16],
            &[
                0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
                0x04, 0x20,
            ]
        );
        assert_eq!(&der[16..], &seed);
    }

    #[test]
    fn pkcs8_pem_armours_correctly() {
        let seed = [0u8; 32];
        let pem = ed25519_pkcs8_pem_from_seed(&seed);
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        assert!(pem.trim_end().ends_with("-----END PRIVATE KEY-----"));
    }

    #[test]
    fn spki_der_round_trips_raw_bytes() {
        let raw = [0x77u8; 32];
        let mut spki = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        spki.extend_from_slice(&raw);
        let got = ed25519_pubkey_raw_from_spki_der(&spki).expect("parse SPKI");
        assert_eq!(got, raw);
    }

    #[test]
    fn spki_der_rejects_wrong_length() {
        let err = ed25519_pubkey_raw_from_spki_der(&[0u8; 43]).unwrap_err();
        matches!(err, OpenSshParseError::WrongPubkeyLength { .. });
    }

    #[test]
    fn spki_der_rejects_non_ed25519_prefix() {
        let mut spki = vec![0xffu8; 44];
        spki[0] = 0x30;
        spki[1] = 0x2a;
        let err = ed25519_pubkey_raw_from_spki_der(&spki).unwrap_err();
        matches!(err, OpenSshParseError::WrongAlgorithm { .. });
    }

    #[test]
    fn fingerprint_matches_manual_computation() {
        let raw = [0x42u8; 32];
        let line = make_openssh_line(&raw);
        let got = fingerprint_openssh_pubkey(&line).expect("fingerprint");
        let expected = {
            let digest = Sha256::digest(raw);
            base64::engine::general_purpose::STANDARD.encode(digest)
        };
        assert_eq!(got, expected);
    }
}

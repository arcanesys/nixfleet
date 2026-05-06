//! Cert issuance for `/v1/enroll` and `/v1/agent/renew`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use nixfleet_proto::enroll_wire::{BootstrapToken, TokenClaims};
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DnType, ExtendedKeyUsagePurpose, KeyPair,
};
use sha2::{Digest, Sha256};

/// 30 days; agents self-pace renewal at 50% via `/v1/agent/renew`.
pub const AGENT_CERT_VALIDITY: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone)]
pub enum AuditContext {
    Enroll { token_nonce: String },
    Renew { previous_cert_serial: String },
}

/// Verifying a bootstrap-token signature involves: read trust.json, parse
/// TrustConfig, walk current+previous orgRootKey candidates, and try ed25519
/// verification against each. Splitting the stages into separate error
/// variants lets axum handlers log + map to StatusCodes correctly without
/// duplicating the candidate loop in every handler.
#[derive(Debug)]
pub enum TrustVerifyError {
    TrustFileRead {
        path: PathBuf,
        source: std::io::Error,
    },
    TrustFileParse {
        source: serde_json::Error,
    },
    NoOrgRootKey,
    NoActiveKeys,
    /// No current/previous orgRootKey candidate verified the signature.
    SignatureMismatch,
}

impl std::fmt::Display for TrustVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrustFileRead { path, source } => {
                write!(f, "read trust.json at {}: {source}", path.display())
            }
            Self::TrustFileParse { source } => write!(f, "parse trust.json: {source}"),
            Self::NoOrgRootKey => write!(
                f,
                "trust.json has no orgRootKey — set nixfleet.trust.orgRootKey.current"
            ),
            Self::NoActiveKeys => write!(f, "orgRootKey has no current/previous keys"),
            Self::SignatureMismatch => write!(
                f,
                "token signature did not verify against any orgRootKey candidate"
            ),
        }
    }
}

/// Loads trust.json fresh on every call so operator key rotations propagate
/// without restart. ed25519-only — non-ed25519 candidates and base64-decode
/// failures are logged and skipped, never fatal.
pub fn verify_bootstrap_token_against_trust(
    trust_path: &Path,
    token: &BootstrapToken,
) -> Result<(), TrustVerifyError> {
    let trust_raw =
        std::fs::read_to_string(trust_path).map_err(|source| TrustVerifyError::TrustFileRead {
            path: trust_path.to_path_buf(),
            source,
        })?;
    let trust: nixfleet_proto::TrustConfig = serde_json::from_str(&trust_raw)
        .map_err(|source| TrustVerifyError::TrustFileParse { source })?;
    let org_root = trust
        .org_root_key
        .as_ref()
        .ok_or(TrustVerifyError::NoOrgRootKey)?;
    let candidates = org_root.active_keys();
    if candidates.is_empty() {
        return Err(TrustVerifyError::NoActiveKeys);
    }

    for pubkey in &candidates {
        if pubkey.algorithm != "ed25519" {
            tracing::warn!(
                algorithm = %pubkey.algorithm,
                "skipping non-ed25519 orgRootKey candidate (only ed25519 supported)",
            );
            continue;
        }
        let pubkey_bytes = match base64::engine::general_purpose::STANDARD.decode(&pubkey.public) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(error = %err, "orgRootKey base64 decode failed; skipping candidate");
                continue;
            }
        };
        if verify_token_signature(token, &pubkey_bytes).is_ok() {
            return Ok(());
        }
    }
    Err(TrustVerifyError::SignatureMismatch)
}

/// Cryptographic signature only; caller handles replay/hostname/fingerprint/expiry.
pub fn verify_token_signature(token: &BootstrapToken, org_root_pubkey: &[u8]) -> Result<()> {
    if token.version != 1 {
        anyhow::bail!("unsupported token version: {}", token.version);
    }
    let pubkey = VerifyingKey::from_bytes(
        org_root_pubkey
            .try_into()
            .context("orgRootKey is not 32 bytes")?,
    )
    .context("parse orgRootKey")?;
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&token.signature)
        .context("decode token signature base64")?;
    let signature = Signature::from_slice(&sig_bytes).context("parse ed25519 signature")?;

    // JCS canonical bytes; matches what the operator-side mint tool signed.
    let claims_json = serde_json::to_string(&token.claims).context("serialize claims")?;
    let canonical =
        nixfleet_canonicalize::canonicalize(&claims_json).context("canonicalize claims")?;
    pubkey
        .verify(canonical.as_bytes(), &signature)
        .context("verify token signature")?;
    Ok(())
}

/// Validates expiry, hostname-vs-CN, and pubkey fingerprint; caller verifies signature/replay.
pub fn validate_token_claims(
    claims: &TokenClaims,
    csr_cn: &str,
    csr_pubkey_fingerprint: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    if now < claims.issued_at {
        anyhow::bail!("token issued in the future");
    }
    if now >= claims.expires_at {
        anyhow::bail!("token expired");
    }
    if csr_cn != claims.hostname {
        anyhow::bail!(
            "CSR CN ({csr_cn}) does not match token hostname ({})",
            claims.hostname
        );
    }
    if csr_pubkey_fingerprint != claims.expected_pubkey_fingerprint {
        anyhow::bail!("CSR pubkey fingerprint does not match token expected_pubkey_fingerprint");
    }
    Ok(())
}

/// Base64(SHA-256(bytes)).
pub fn fingerprint(pubkey_bytes: &[u8]) -> String {
    let digest = Sha256::digest(pubkey_bytes);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Extract the first private-key PEM block from a (possibly multi-block)
/// PEM document. Re-emits as a single-block PEM string suitable for
/// `rcgen::KeyPair::from_pem`.
///
/// Why this exists: OpenSSL-generated EC keys (`openssl ecparam
/// -genkey`) write TWO blocks — a leading `EC PARAMETERS` block (just
/// the curve OID) and the actual `EC PRIVATE KEY`. rcgen's
/// `KeyPair::from_pem` reads only the first block, sees the parameters
/// block, fails. PKCS#8-formatted keys (`BEGIN PRIVATE KEY`) and
/// rcgen-generated keys are single-block and work without this
/// helper, but operators routinely supply OpenSSL-generated CAs in
/// production. Accepting the wider PEM shape keeps the framework
/// robust to whatever shape the operator's offline CA tooling emits.
///
/// Accepted labels (in priority order): `PRIVATE KEY` (PKCS#8),
/// `EC PRIVATE KEY` (SEC1), `RSA PRIVATE KEY` (PKCS#1).
pub fn extract_private_key_pem_block(pem_text: &str) -> Result<String> {
    const ACCEPTED: &[&str] = &["PRIVATE KEY", "EC PRIVATE KEY", "RSA PRIVATE KEY"];

    let mut current_label: Option<String> = None;
    let mut current_body = String::new();

    for line in pem_text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("-----BEGIN ")
            .and_then(|s| s.strip_suffix("-----"))
        {
            current_label = Some(rest.to_string());
            current_body.clear();
        } else if let Some(rest) = trimmed
            .strip_prefix("-----END ")
            .and_then(|s| s.strip_suffix("-----"))
        {
            if let Some(start) = current_label.take() {
                if start == rest && ACCEPTED.iter().any(|&l| l == start) {
                    return Ok(format!(
                        "-----BEGIN {start}-----\n{body}-----END {start}-----\n",
                        body = current_body,
                    ));
                }
            }
            current_body.clear();
        } else if current_label.is_some() {
            current_body.push_str(line);
            current_body.push('\n');
        }
    }

    anyhow::bail!(
        "no PEM block matching {ACCEPTED:?} found — supply a PKCS#8 \
         (`BEGIN PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE KEY`) key",
    )
}

/// Validates that the CSR's raw ed25519 pubkey matches the host's
/// declared SSH host pubkey (`hosts.<hostname>.pubkey` from
/// fleet.resolved.json). Closes RFC-0003 §2: agent identity is bound
/// to the SSH host key, not a fresh keypair.
///
/// Fail-closed: a host with no `pubkey` declared in fleet.nix CANNOT
/// enroll. The expected workflow is "operator declares host (with
/// pubkey) in fleet.nix → CI signs new fleet.resolved → agent enrols"
/// — there's no permissive fallback.
pub fn validate_csr_against_fleet_host(
    csr_pubkey_raw: &[u8],
    declared_openssh_pubkey: Option<&str>,
) -> Result<()> {
    let openssh = declared_openssh_pubkey.ok_or_else(|| {
        anyhow::anyhow!(
            "host has no `pubkey` declared in fleet.nix — \
             enrollment refused (declarative-enrollment policy)"
        )
    })?;
    let declared_raw =
        nixfleet_proto::host_key::ed25519_pubkey_raw_from_openssh(openssh)
            .with_context(|| {
                format!("parse declared OpenSSH pubkey for fleet host: {openssh}")
            })?;
    if csr_pubkey_raw != declared_raw {
        anyhow::bail!(
            "CSR pubkey does not match host's declared SSH host pubkey \
             (CSR fingerprint: {}, declared fingerprint: {})",
            fingerprint(csr_pubkey_raw),
            fingerprint(&declared_raw),
        );
    }
    Ok(())
}

/// Issues an agent cert (clientAuth EKU + SAN dNSName=CN); caller pre-validates CN.
pub fn issue_cert(
    csr_pem: &str,
    ca_cert_path: &Path,
    ca_key_path: &Path,
    validity: Duration,
    now: DateTime<Utc>,
) -> Result<(String, DateTime<Utc>)> {
    let ca_cert_pem = std::fs::read_to_string(ca_cert_path)
        .with_context(|| format!("read fleet CA cert {}", ca_cert_path.display()))?;
    let ca_key_pem_raw = std::fs::read_to_string(ca_key_path)
        .with_context(|| format!("read fleet CA key {}", ca_key_path.display()))?;
    // FOOTGUN: rcgen's `from_pem` reads only the first block. OpenSSL
    // EC keys ship `EC PARAMETERS` first and `EC PRIVATE KEY` second —
    // strip the parameters block before handing to rcgen.
    let ca_key_pem = extract_private_key_pem_block(&ca_key_pem_raw)
        .context("extract private-key block from fleet CA key PEM")?;
    let ca_key = KeyPair::from_pem(&ca_key_pem).context("parse fleet CA key PEM")?;
    let ca_params =
        CertificateParams::from_ca_cert_pem(&ca_cert_pem).context("parse fleet CA cert PEM")?;
    let ca = ca_params
        .self_signed(&ca_key)
        .context("rebuild fleet CA from PEM (rcgen quirk)")?;

    let csr_params = CertificateSigningRequestParams::from_pem(csr_pem).context("parse CSR PEM")?;
    let cn = csr_params
        .params
        .distinguished_name
        .iter()
        .find_map(|(t, v): (&DnType, &rcgen::DnValue)| {
            if matches!(t, DnType::CommonName) {
                Some(v.clone())
            } else {
                None
            }
        })
        .context("CSR has no CN")?;

    let mut params = csr_params.params;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    // FOOTGUN: rustls/webpki rejects CN-only certs — SAN dNSName=CN is required for mTLS to work.
    let cn_str = match &cn {
        rcgen::DnValue::PrintableString(s) => s.to_string(),
        rcgen::DnValue::Utf8String(s) => s.to_string(),
        _ => format!("{:?}", cn),
    };
    params.subject_alt_names = vec![rcgen::SanType::DnsName(
        cn_str
            .clone()
            .try_into()
            .context("CN is not a valid dNSName")?,
    )];

    let not_before_sys = SystemTime::UNIX_EPOCH + Duration::from_secs(now.timestamp() as u64);
    let not_after_sys = not_before_sys + validity;
    params.not_before = not_before_sys.into();
    params.not_after = not_after_sys.into();

    let cert = params
        .signed_by(&csr_params.public_key, &ca, &ca_key)
        .context("sign cert with fleet CA")?;

    let not_after = chrono::DateTime::<Utc>::from(not_after_sys);
    Ok((cert.pem(), not_after))
}

/// Best-effort append; write failure warns but doesn't fail issuance.
pub fn audit_log(
    path: &Path,
    now: DateTime<Utc>,
    requester_cn: &str,
    issued_cn: &str,
    not_after: DateTime<Utc>,
    context: &AuditContext,
) {
    let context_str = match context {
        AuditContext::Enroll { token_nonce } => format!("enroll/nonce:{token_nonce}"),
        AuditContext::Renew {
            previous_cert_serial,
        } => format!("renew/prev:{previous_cert_serial}"),
    };
    let record = serde_json::json!({
        "at": now.to_rfc3339(),
        "requester_cn": requester_cn,
        "issued_cn": issued_cn,
        "not_after": not_after.to_rfc3339(),
        "context": context_str,
    });
    let line = serde_json::to_string(&record)
        .expect("serde_json::to_string on a json!() Value is infallible");
    if let Err(err) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{line}")
        })
    {
        tracing::warn!(error = %err, path = %path.display(), "failed to append audit log");
    }
}

#[cfg(test)]
mod validate_csr_tests {
    use super::*;
    use base64::Engine;

    /// Build a valid OpenSSH ed25519 pubkey line wrapping `raw`.
    fn openssh_line(raw: &[u8; 32]) -> String {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        blob.extend_from_slice(raw);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        format!("ssh-ed25519 {b64} test@host")
    }

    #[test]
    fn accepts_when_csr_pubkey_matches_declared() {
        let raw = [0x42u8; 32];
        let declared = openssh_line(&raw);
        validate_csr_against_fleet_host(&raw, Some(&declared)).expect("should accept match");
    }

    #[test]
    fn rejects_when_csr_pubkey_differs() {
        let csr_raw = [0x42u8; 32];
        let declared_raw = [0x43u8; 32];
        let declared = openssh_line(&declared_raw);
        let err = validate_csr_against_fleet_host(&csr_raw, Some(&declared)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("does not match"), "msg = {msg}");
    }

    #[test]
    fn rejects_when_no_pubkey_declared() {
        let csr_raw = [0x42u8; 32];
        let err = validate_csr_against_fleet_host(&csr_raw, None).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("declarative-enrollment policy"),
            "msg = {msg}",
        );
    }

    #[test]
    fn rejects_when_declared_pubkey_unparseable() {
        let csr_raw = [0x42u8; 32];
        let err =
            validate_csr_against_fleet_host(&csr_raw, Some("ssh-rsa garbage")).unwrap_err();
        let msg = format!("{err}");
        // The error chain mentions the parse failure context.
        assert!(msg.contains("parse declared"), "msg = {msg}");
    }
}

#[cfg(test)]
mod extract_pem_tests {
    use super::*;

    #[test]
    fn accepts_single_block_pkcs8() {
        // PKCS#8 PEM produced by rcgen / mkcert / openssl pkcs8 -topk8.
        // Body is opaque to the extractor — it just needs the labels.
        let input = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
        let got = extract_private_key_pem_block(input).expect("PKCS#8 single block");
        assert!(got.contains("-----BEGIN PRIVATE KEY-----"));
        assert!(got.contains("AAAA"));
    }

    #[test]
    fn accepts_single_block_sec1() {
        let input = "-----BEGIN EC PRIVATE KEY-----\nBBBB\n-----END EC PRIVATE KEY-----\n";
        let got = extract_private_key_pem_block(input).expect("SEC1 single block");
        assert!(got.starts_with("-----BEGIN EC PRIVATE KEY-----"));
    }

    #[test]
    fn extracts_key_block_from_multi_block_openssl_ec() {
        // Shape of `openssl ecparam -genkey -name prime256v1` output:
        // EC PARAMETERS first (curve OID), then EC PRIVATE KEY.
        let input = "\
-----BEGIN EC PARAMETERS-----
BggqhkjOPQMBBw==
-----END EC PARAMETERS-----
-----BEGIN EC PRIVATE KEY-----
MHcCAQEEIBoldKey...
-----END EC PRIVATE KEY-----
";
        let got = extract_private_key_pem_block(input).expect("multi-block extract");
        assert!(got.starts_with("-----BEGIN EC PRIVATE KEY-----"));
        assert!(!got.contains("EC PARAMETERS"), "must drop the parameters block");
        assert!(got.contains("MHcCAQEEIBoldKey"));
    }

    #[test]
    fn rejects_pem_with_no_key_block() {
        let input = "\
-----BEGIN EC PARAMETERS-----
BggqhkjOPQMBBw==
-----END EC PARAMETERS-----
";
        let err = extract_private_key_pem_block(input).expect_err("no key block");
        let msg = format!("{err}");
        assert!(
            msg.contains("PRIVATE KEY"),
            "msg should mention accepted labels: {msg}",
        );
    }

    #[test]
    fn rejects_garbage_input() {
        let err = extract_private_key_pem_block("not a pem file at all").expect_err("not PEM");
        let msg = format!("{err}");
        assert!(msg.contains("PRIVATE KEY"));
    }

    #[test]
    fn ignores_unrelated_block_types() {
        // `BEGIN CERTIFICATE` is a non-key block. Must not be returned
        // as if it were a key.
        let input = "\
-----BEGIN CERTIFICATE-----
ZHVtbXk=
-----END CERTIFICATE-----
";
        let err = extract_private_key_pem_block(input).expect_err("certificate is not a key");
        assert!(format!("{err}").contains("PRIVATE KEY"));
    }

    #[test]
    fn returns_first_matching_block_when_multiple_keys_present() {
        // Defensive: file with two PRIVATE KEY blocks (would be unusual
        // but possible). Take the first.
        let input = "\
-----BEGIN PRIVATE KEY-----
FIRST
-----END PRIVATE KEY-----
-----BEGIN PRIVATE KEY-----
SECOND
-----END PRIVATE KEY-----
";
        let got = extract_private_key_pem_block(input).expect("first key");
        assert!(got.contains("FIRST"));
        assert!(!got.contains("SECOND"));
    }

    #[test]
    fn round_trips_rcgen_generated_pkcs8() {
        // Sanity: keys produced by rcgen still parse after going
        // through the extractor (round-trip).
        use rcgen::KeyPair;
        let key = KeyPair::generate().expect("rcgen generate");
        let pem = key.serialize_pem();
        let extracted = extract_private_key_pem_block(&pem).expect("rcgen pem");
        let _reparsed = KeyPair::from_pem(&extracted).expect("rcgen round-trip");
    }
}

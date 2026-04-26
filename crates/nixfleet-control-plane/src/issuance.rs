//! Cert issuance for `/v1/enroll` and `/v1/agent/renew`.
//!
//! Validates the CSR + token, builds a TBS certificate with the
//! standard agent-cert profile (clientAuth EKU, SAN dNSName), and
//! signs with the fleet CA's private key. **The fleet CA private
//! key is read at issuance time from a path on disk — issue #41
//! tracks moving it to TPM-bound signing.**
//!
//! Audit log: every issuance writes one JSON line to journal AND
//! appends to a configured audit-log file. The file is plaintext
//! JSON-lines (one record per line) so an operator can `tail -f`
//! it during incidents.

use std::path::Path;
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

/// 30 days — D6 default. Agent self-paces renewal at 50% via
/// `/v1/agent/renew`.
pub const AGENT_CERT_VALIDITY: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Audit context attached to every issuance record. Distinguishes
/// /enroll from /renew in the audit log so operators can grep
/// post-incident.
#[derive(Debug, Clone)]
pub enum AuditContext {
    Enroll {
        token_nonce: String,
    },
    Renew {
        previous_cert_serial: String,
    },
}

/// In-memory replay set for bootstrap-token nonces. PR-5 wraps this
/// in `Arc<RwLock<HashSet<String>>>` inside AppState.
pub fn token_seen(nonces: &std::collections::HashSet<String>, nonce: &str) -> bool {
    nonces.contains(nonce)
}

/// Verify a bootstrap token's signature against the org root key.
/// Caller is responsible for: nonce-replay check, hostname match,
/// expected-pubkey-fingerprint match, expiry check. This function
/// only validates the cryptographic signature.
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

    // Canonical bytes of `claims` per JCS — this is what the
    // operator-side mint tool signs.
    let claims_json = serde_json::to_string(&token.claims).context("serialize claims")?;
    let canonical =
        nixfleet_canonicalize::canonicalize(&claims_json).context("canonicalize claims")?;
    pubkey
        .verify(canonical.as_bytes(), &signature)
        .context("verify token signature")?;
    Ok(())
}

/// Validate the typed parts of a token's claims (expiry, hostname-vs-CN,
/// pubkey fingerprint). Pure function — caller has already verified
/// the signature and replay status.
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
        anyhow::bail!(
            "CSR pubkey fingerprint does not match token expected_pubkey_fingerprint"
        );
    }
    Ok(())
}

/// SHA-256 of an SPKI DER (or any pubkey byte representation), base64-
/// encoded. Caller decides what bytes to feed in — we just hash.
pub fn fingerprint(pubkey_bytes: &[u8]) -> String {
    let digest = Sha256::digest(pubkey_bytes);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Issue a signed agent certificate.
///
/// The CSR is parsed; the new cert inherits the CSR's subject DN
/// + pubkey, gets a clientAuth EKU, a SAN dNSName matching the CN
/// (rustls/webpki rejects CN-only certs), and the configured
/// validity. Signed with the fleet CA private key loaded from
/// `ca_key_path`.
///
/// Caller is expected to have validated the CSR's CN already; this
/// function does not double-check.
pub fn issue_cert(
    csr_pem: &str,
    ca_cert_path: &Path,
    ca_key_path: &Path,
    validity: Duration,
    now: DateTime<Utc>,
) -> Result<(String, DateTime<Utc>)> {
    // Load the fleet CA cert + private key.
    let ca_cert_pem = std::fs::read_to_string(ca_cert_path)
        .with_context(|| format!("read fleet CA cert {}", ca_cert_path.display()))?;
    let ca_key_pem = std::fs::read_to_string(ca_key_path)
        .with_context(|| format!("read fleet CA key {}", ca_key_path.display()))?;
    let ca_key = KeyPair::from_pem(&ca_key_pem).context("parse fleet CA key PEM")?;
    let ca_params = CertificateParams::from_ca_cert_pem(&ca_cert_pem)
        .context("parse fleet CA cert PEM")?;
    let ca = ca_params
        .self_signed(&ca_key)
        .context("rebuild fleet CA from PEM (rcgen quirk)")?;

    // Parse the CSR into params we can re-emit signed.
    let csr_params = CertificateSigningRequestParams::from_pem(csr_pem)
        .context("parse CSR PEM")?;
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
    // Add SAN dNSName matching the CN. rustls/webpki rejects CN-only
    // certs, so without this the cert is unusable for mTLS.
    let cn_str = match &cn {
        rcgen::DnValue::PrintableString(s) => s.to_string(),
        rcgen::DnValue::Utf8String(s) => s.to_string(),
        _ => format!("{:?}", cn),
    };
    params.subject_alt_names = vec![rcgen::SanType::DnsName(
        cn_str.clone().try_into().context("CN is not a valid dNSName")?,
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

/// Append one JSON line to the audit log file. Best-effort —
/// failure to write the audit log warns but does not fail the
/// issuance (the journal still has a tracing record).
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
    let line = serde_json::to_string(&record).unwrap_or_default();
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

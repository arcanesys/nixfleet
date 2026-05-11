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

/// Default suffix for canonical agent CNs (`agent-<machineId>.<suffix>`).
/// Must match the issuance CA's `dNSName` constraint (D14).
pub const DEFAULT_AGENT_CN_SUFFIX: &str = "fleet.lab.internal";

/// Build the canonical CN for an agent cert: `agent-<machineId>.<suffix>`.
pub fn canonical_agent_cn(machine_id: &str, suffix: &str) -> String {
    format!("agent-{machine_id}.{suffix}")
}

/// Idempotent: passes through bare CNs unchanged, strips canonical wrapper.
pub fn extract_machine_id(cn: &str, suffix: &str) -> String {
    let trailer = format!(".{suffix}");
    if let Some(rest) = cn.strip_prefix("agent-") {
        if let Some(machine_id) = rest.strip_suffix(&trailer) {
            return machine_id.to_string();
        }
    }
    cn.to_string()
}

#[derive(Debug, Clone)]
pub enum AuditContext {
    Enroll { token_nonce: String },
    Renew { previous_cert_serial: String },
}

/// Stage-typed error so axum handlers map each phase to the right StatusCode.
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
                "trust.json has no orgRootKey - set nixfleet.trust.orgRootKey.current"
            ),
            Self::NoActiveKeys => write!(f, "orgRootKey has no current/previous keys"),
            Self::SignatureMismatch => write!(
                f,
                "token signature did not verify against any orgRootKey candidate"
            ),
        }
    }
}

/// Re-reads trust.json per call (key rotations apply without restart).
/// ed25519-only. `now` enables the `successor` overlap window.
pub fn verify_bootstrap_token_against_trust(
    trust_path: &Path,
    token: &BootstrapToken,
    now: DateTime<Utc>,
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
    let candidates = org_root.active_keys_at(now);
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

    // JCS canonical bytes match what the operator-side mint tool signed.
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

/// Extract the first private-key PEM block. Strips OpenSSL preambles
/// (`EC PARAMETERS`) that rcgen's `KeyPair::from_pem` can't read.
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
        "no PEM block matching {ACCEPTED:?} found - supply a PKCS#8 \
         (`BEGIN PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE KEY`) key",
    )
}

/// Bind agent identity to the host's declared SSH host pubkey. Fail-closed:
/// no `pubkey` in fleet.nix ⇒ enrollment refused (declarative-enrollment
/// policy, no permissive fallback).
pub fn validate_csr_against_fleet_host(
    csr_pubkey_raw: &[u8],
    declared_openssh_pubkey: Option<&str>,
) -> Result<()> {
    let openssh = declared_openssh_pubkey.ok_or_else(|| {
        anyhow::anyhow!(
            "host has no `pubkey` declared in fleet.nix - \
             enrollment refused (declarative-enrollment policy)"
        )
    })?;
    let declared_raw = nixfleet_proto::host_key::ed25519_pubkey_raw_from_openssh(openssh)
        .with_context(|| format!("parse declared OpenSSH pubkey for fleet host: {openssh}"))?;
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

/// CA signer abstraction over file-backed + TPM-backed paths. `issuer()`
/// caches the cert at construction; `make_key_pair()` produces a fresh signer
/// per issuance so operator key rotations apply without restart.
pub trait CaSigner: Send + Sync {
    fn issuer(&self) -> &rcgen::Certificate;
    fn make_key_pair(&self) -> Result<KeyPair>;
}

/// Builds a `CaSigner` from a flag triple. TPM (`pub_raw + wrapper`) wins
/// over file (`fleet_ca_key`); both absent → `None` (enroll/renew will 500).
pub fn build_signer_from_args(
    cert_path: &Path,
    tpm_pubkey_raw: Option<&Path>,
    tpm_sign_wrapper: Option<&Path>,
    fleet_ca_key: Option<&Path>,
) -> Option<std::sync::Arc<dyn CaSigner>> {
    match (tpm_pubkey_raw, tpm_sign_wrapper, fleet_ca_key) {
        (Some(pub_raw), Some(wrapper), _) => {
            match TpmCaSigner::from_paths(cert_path, pub_raw, wrapper) {
                Ok(s) => {
                    tracing::info!(
                        cert = %cert_path.display(),
                        pubkey_raw = %pub_raw.display(),
                        wrapper = %wrapper.display(),
                        "issuance CA signer: TPM-backed",
                    );
                    Some(std::sync::Arc::new(s))
                }
                Err(err) => {
                    tracing::error!(error = %err, "build TPM CA signer; enroll/renew will 500");
                    None
                }
            }
        }
        (None, None, Some(key_path)) => match FileCaSigner::from_paths(cert_path, key_path) {
            Ok(s) => {
                tracing::info!(
                    cert = %cert_path.display(),
                    key = %key_path.display(),
                    "issuance CA signer: file-backed",
                );
                Some(std::sync::Arc::new(s))
            }
            Err(err) => {
                tracing::error!(error = %err, "build file CA signer; enroll/renew will 500");
                None
            }
        },
        _ => {
            tracing::warn!(
                "no CA signer flags satisfied (need --fleet-ca-key or --tpm-ca-pubkey-raw + \
                 --tpm-ca-sign-wrapper); enroll/renew will 500"
            );
            None
        }
    }
}

/// File-backed CA signer (current default; agenix-encrypted PEM on disk).
pub struct FileCaSigner {
    key_path: PathBuf,
    issuer_cert: rcgen::Certificate,
}

impl FileCaSigner {
    pub fn from_paths(ca_cert_path: &Path, ca_key_path: &Path) -> Result<Self> {
        let ca_cert_pem = std::fs::read_to_string(ca_cert_path)
            .with_context(|| format!("read fleet CA cert {}", ca_cert_path.display()))?;
        let ca_key_pem_raw = std::fs::read_to_string(ca_key_path)
            .with_context(|| format!("read fleet CA key {}", ca_key_path.display()))?;
        // FOOTGUN: rcgen's `from_pem` reads only the first block. OpenSSL
        // EC keys ship `EC PARAMETERS` first and `EC PRIVATE KEY` second  -
        // strip the parameters block before handing to rcgen.
        let ca_key_pem = extract_private_key_pem_block(&ca_key_pem_raw)
            .context("extract private-key block from fleet CA key PEM")?;
        let ca_key = KeyPair::from_pem(&ca_key_pem).context("parse fleet CA key PEM")?;
        let ca_params =
            CertificateParams::from_ca_cert_pem(&ca_cert_pem).context("parse fleet CA cert PEM")?;
        let issuer_cert = ca_params
            .self_signed(&ca_key)
            .context("rebuild fleet CA from PEM (rcgen quirk)")?;
        Ok(Self {
            key_path: ca_key_path.to_path_buf(),
            issuer_cert,
        })
    }
}

impl CaSigner for FileCaSigner {
    fn issuer(&self) -> &rcgen::Certificate {
        &self.issuer_cert
    }
    fn make_key_pair(&self) -> Result<KeyPair> {
        let raw = std::fs::read_to_string(&self.key_path)
            .with_context(|| format!("read fleet CA key {}", self.key_path.display()))?;
        let pem = extract_private_key_pem_block(&raw).context("extract CA key PEM block")?;
        KeyPair::from_pem(&pem).context("parse fleet CA key PEM")
    }
}

/// TPM-backed CA. Holds uncompressed SEC1 P-256 pubkey (`0x04 || X || Y`,
/// 65 bytes - rcgen's ECDSA shape) + the `tpm-sign-<keyname>` wrapper path.
/// Issuer cert is self-signed via TPM once at construction; the real CA
/// signature (by the offline fleet root) lives on disk and rcgen never
/// reads it (only DN + pubkey), so the re-self-sign is sound.
pub struct TpmCaSigner {
    pubkey_uncompressed: Vec<u8>,
    sign_wrapper_path: PathBuf,
    issuer_cert: rcgen::Certificate,
}

impl TpmCaSigner {
    /// `tpm_pubkey_raw_path` = 64-byte X||Y (no leading 0x04).
    /// `sign_wrapper_path` = `tpm-sign-<keyname>` binary.
    pub fn from_paths(
        ca_cert_path: &Path,
        tpm_pubkey_raw_path: &Path,
        sign_wrapper_path: &Path,
    ) -> Result<Self> {
        let pubkey_raw = std::fs::read(tpm_pubkey_raw_path)
            .with_context(|| format!("read TPM pubkey {}", tpm_pubkey_raw_path.display()))?;
        if pubkey_raw.len() != 64 {
            anyhow::bail!(
                "TPM pubkey expected 64 bytes (raw P-256 X||Y), got {}",
                pubkey_raw.len(),
            );
        }
        let mut pubkey_uncompressed = Vec::with_capacity(65);
        pubkey_uncompressed.push(0x04);
        pubkey_uncompressed.extend_from_slice(&pubkey_raw);

        let ca_cert_pem = std::fs::read_to_string(ca_cert_path)
            .with_context(|| format!("read issuance CA cert {}", ca_cert_path.display()))?;

        // One TPM sign at startup to produce the in-memory issuer Cert.
        let signer = TpmRemoteSigner {
            pubkey_uncompressed: pubkey_uncompressed.clone(),
            sign_wrapper_path: sign_wrapper_path.to_path_buf(),
        };
        let key_pair = KeyPair::from_remote(Box::new(signer)).context("rcgen from_remote (TPM)")?;
        let ca_params = CertificateParams::from_ca_cert_pem(&ca_cert_pem)
            .context("parse issuance CA cert PEM")?;
        let issuer_cert = ca_params
            .self_signed(&key_pair)
            .context("self-sign issuer via TPM at startup")?;

        Ok(Self {
            pubkey_uncompressed,
            sign_wrapper_path: sign_wrapper_path.to_path_buf(),
            issuer_cert,
        })
    }
}

impl CaSigner for TpmCaSigner {
    fn issuer(&self) -> &rcgen::Certificate {
        &self.issuer_cert
    }
    fn make_key_pair(&self) -> Result<KeyPair> {
        let signer = TpmRemoteSigner {
            pubkey_uncompressed: self.pubkey_uncompressed.clone(),
            sign_wrapper_path: self.sign_wrapper_path.clone(),
        };
        KeyPair::from_remote(Box::new(signer)).context("rcgen from_remote (TPM)")
    }
}

/// Shells out to the keyslot's `tpm-sign-<keyname>` (file in, raw R||S out);
/// `der_encode_ecdsa_p256_sig` rewraps to the DER form rcgen expects.
struct TpmRemoteSigner {
    pubkey_uncompressed: Vec<u8>,
    sign_wrapper_path: PathBuf,
}

impl rcgen::RemoteKeyPair for TpmRemoteSigner {
    fn public_key(&self) -> &[u8] {
        &self.pubkey_uncompressed
    }
    fn sign(&self, msg: &[u8]) -> std::result::Result<Vec<u8>, rcgen::Error> {
        let raw = invoke_tpm_sign(&self.sign_wrapper_path, msg).map_err(|err| {
            tracing::error!(error = %err, "tpm-sign invocation failed");
            rcgen::Error::RingUnspecified
        })?;
        der_encode_ecdsa_p256_sig(&raw).map_err(|err| {
            tracing::error!(error = %err, raw_len = raw.len(), "DER-encoding TPM signature failed");
            rcgen::Error::RingUnspecified
        })
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}

/// Write `msg` to a tempfile, invoke the tpm-sign wrapper, return stdout.
fn invoke_tpm_sign(wrapper: &Path, msg: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().context("create tpm-sign tempfile")?;
    tmp.write_all(msg).context("write tpm-sign tempfile")?;
    tmp.flush().ok();
    let output = std::process::Command::new(wrapper)
        .arg(tmp.path())
        .output()
        .with_context(|| format!("invoke tpm-sign wrapper {}", wrapper.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "tpm-sign wrapper {} exited {}: stderr={}",
            wrapper.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(output.stdout)
}

/// Convert raw `R || S` (64 bytes for P-256) to DER `Ecdsa-Sig-Value
/// SEQUENCE { r INTEGER, s INTEGER }` - what rcgen expects from a
/// `RemoteKeyPair::sign` for ECDSA P-256.
fn der_encode_ecdsa_p256_sig(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() != 64 {
        anyhow::bail!("expected 64 raw P-256 sig bytes, got {}", raw.len());
    }
    let r = der_encode_int(&raw[..32]);
    let s = der_encode_int(&raw[32..]);
    let body_len = r.len() + s.len();
    // SEQUENCE for P-256 worst case: 2*(2+33) = 70 body, +2 header = 72; <128
    // so always single-byte length.
    let mut out = Vec::with_capacity(2 + body_len);
    out.push(0x30); // SEQUENCE
    out.push(body_len as u8);
    out.extend(r);
    out.extend(s);
    Ok(out)
}

/// Big-endian unsigned int → DER INTEGER. Strips leading zeros + pads if MSB set.
fn der_encode_int(bytes: &[u8]) -> Vec<u8> {
    let mut start = 0;
    while start + 1 < bytes.len() && bytes[start] == 0 {
        start += 1;
    }
    let needs_pad = (bytes[start] & 0x80) != 0;
    let len = bytes.len() - start + usize::from(needs_pad);
    let mut out = Vec::with_capacity(2 + len);
    out.push(0x02); // INTEGER
    out.push(len as u8);
    if needs_pad {
        out.push(0x00);
    }
    out.extend_from_slice(&bytes[start..]);
    out
}

/// Issues an agent cert: clientAuth EKU + canonical CN `agent-<machineId>.<suffix>`
/// + SAN dNSName=<CN> (rustls/webpki rejects CN-only certs). CSR CN is read as
/// the bare machineId. Caller validates CSR-pubkey ↔ host-pubkey binding upstream.
pub fn issue_cert(
    csr_pem: &str,
    signer: &dyn CaSigner,
    validity: Duration,
    now: DateTime<Utc>,
    agent_cn_suffix: &str,
) -> Result<(String, DateTime<Utc>)> {
    let ca_key = signer.make_key_pair()?;
    let ca = signer.issuer();

    let csr_params = CertificateSigningRequestParams::from_pem(csr_pem).context("parse CSR PEM")?;
    let csr_cn = csr_params
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

    let csr_cn_str = match &csr_cn {
        rcgen::DnValue::PrintableString(s) => s.to_string(),
        rcgen::DnValue::Utf8String(s) => s.to_string(),
        _ => format!("{:?}", csr_cn),
    };
    // CSR CN = bare machineId; we rewrite to canonical FQDN for the D14
    // name constraint. `extract_machine_id` is idempotent on canonical input.
    let machine_id = extract_machine_id(&csr_cn_str, agent_cn_suffix);
    let canonical_cn = canonical_agent_cn(&machine_id, agent_cn_suffix);

    let mut params = csr_params.params;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

    // Preserve non-CN attributes (O, OU, C) the CSR carried.
    let mut new_dn = rcgen::DistinguishedName::new();
    for (t, v) in params.distinguished_name.iter() {
        if !matches!(t, DnType::CommonName) {
            new_dn.push(t.clone(), v.clone());
        }
    }
    new_dn.push(DnType::CommonName, &*canonical_cn);
    params.distinguished_name = new_dn;

    // FOOTGUN: rustls/webpki rejects CN-only certs - SAN dNSName=CN is required for mTLS to work.
    params.subject_alt_names = vec![rcgen::SanType::DnsName(
        canonical_cn
            .clone()
            .try_into()
            .context("canonical CN is not a valid dNSName")?,
    )];

    let not_before_sys = SystemTime::UNIX_EPOCH + Duration::from_secs(now.timestamp() as u64);
    let not_after_sys = not_before_sys + validity;
    params.not_before = not_before_sys.into();
    params.not_after = not_after_sys.into();

    let cert = params
        .signed_by(&csr_params.public_key, ca, &ca_key)
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
mod cn_helpers_tests {
    use super::*;

    #[test]
    fn canonicalises_bare_machine_id() {
        assert_eq!(
            canonical_agent_cn("krach", "fleet.lab.internal"),
            "agent-krach.fleet.lab.internal"
        );
    }

    #[test]
    fn extracts_machine_id_from_canonical_cn() {
        assert_eq!(
            extract_machine_id("agent-krach.fleet.lab.internal", "fleet.lab.internal"),
            "krach"
        );
    }

    #[test]
    fn extract_passes_through_legacy_bare_cn() {
        // Pre-C.3 cert: CN=<machineId>, no FQDN. Must pass through
        // unchanged so the renew handler's fleet.hosts lookup still
        // works during the migration window.
        assert_eq!(extract_machine_id("krach", "fleet.lab.internal"), "krach");
    }

    #[test]
    fn extract_passes_through_when_suffix_does_not_match() {
        // CN under an unexpected suffix → treat as opaque, return as-is.
        // Defensive: prevents accidental machineId collisions across
        // suffixes (operator running two fleets with overlapping IDs).
        assert_eq!(
            extract_machine_id("agent-krach.other.example", "fleet.lab.internal"),
            "agent-krach.other.example"
        );
    }

    #[test]
    fn canonicalisation_roundtrips_through_extract() {
        for id in ["krach", "ohm", "pixel", "machine-with-dashes"] {
            let canonical = canonical_agent_cn(id, "fleet.lab.internal");
            assert_eq!(
                extract_machine_id(&canonical, "fleet.lab.internal"),
                id,
                "round-trip failed for {id}"
            );
        }
    }
}

#[cfg(test)]
mod der_encode_tests {
    use super::*;

    #[test]
    fn der_encode_int_handles_padding_and_stripping() {
        // (label, input, expected)
        let cases: &[(&str, &[u8], &[u8])] = &[
            (
                "minimal positive: no pad/strip",
                &[0x01],
                &[0x02, 0x01, 0x01],
            ),
            ("MSB set: prepend 0x00", &[0x80], &[0x02, 0x02, 0x00, 0x80]),
            (
                "strip leading zeros",
                &[0x00, 0x00, 0x42],
                &[0x02, 0x01, 0x42],
            ),
            // DER INTEGER 0 must be a single 0x00 byte.
            (
                "zero stays as 0x00",
                &[0x00, 0x00, 0x00],
                &[0x02, 0x01, 0x00],
            ),
            (
                "strip then pad when MSB still set",
                &[0x00, 0x80, 0x01],
                &[0x02, 0x03, 0x00, 0x80, 0x01],
            ),
        ];
        for (label, input, expected) in cases {
            assert_eq!(der_encode_int(input).as_slice(), *expected, "case: {label}");
        }
    }

    #[test]
    fn rejects_wrong_length_p256_sig() {
        let err = der_encode_ecdsa_p256_sig(&[0u8; 32]).unwrap_err();
        assert!(format!("{err}").contains("64"));
    }

    #[test]
    fn encodes_p256_sig_typical_shape() {
        // Mid-range r and s without MSB set - clean encoding, no padding.
        let mut raw = [0u8; 64];
        raw[..32]
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = 1 + i as u8);
        raw[32..]
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = 0x40 + i as u8);
        let der = der_encode_ecdsa_p256_sig(&raw).expect("encode");
        // SEQUENCE
        assert_eq!(der[0], 0x30);
        // body length: 2 (INTEGER tag+len) + 32 + 2 + 32 = 68
        assert_eq!(der[1], 68);
        // first INTEGER
        assert_eq!(der[2], 0x02);
        assert_eq!(der[3], 32);
        // second INTEGER
        assert_eq!(der[2 + 2 + 32], 0x02);
        assert_eq!(der[2 + 2 + 32 + 1], 32);
    }

    #[test]
    fn encodes_p256_sig_max_padding_both_components() {
        // MSB set in both r and s → both pad to 33 bytes.
        let mut raw = [0u8; 64];
        raw[0] = 0x80; // r MSB set
        raw[32] = 0x80; // s MSB set
        let der = der_encode_ecdsa_p256_sig(&raw).expect("encode");
        // body: 2 + 33 + 2 + 33 = 70
        assert_eq!(der[1], 70);
        // first INTEGER length 33
        assert_eq!(der[3], 33);
        // padding byte
        assert_eq!(der[4], 0x00);
        assert_eq!(der[5], 0x80);
        // second INTEGER length 33
        assert_eq!(der[2 + 2 + 33], 0x02);
        assert_eq!(der[2 + 2 + 33 + 1], 33);
    }
}

#[cfg(test)]
mod ca_signer_tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair as RcgenKeyPair};

    /// FileCaSigner round-trip: build, issue an agent cert, verify the
    /// resulting PEM parses as an X.509 cert with the expected subject DN.
    #[test]
    fn file_ca_signer_round_trips() {
        // Mint a throwaway P-256 CA for the test.
        let ca_key = RcgenKeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .expect("rcgen P-256 keypair");
        let mut ca_params = CertificateParams::new(vec!["test-ca".to_string()]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_cert = ca_params.self_signed(&ca_key).expect("self-sign");

        let tmp = tempfile::tempdir().unwrap();
        let cert_path = tmp.path().join("ca.pem");
        let key_path = tmp.path().join("ca.key");
        std::fs::write(&cert_path, ca_cert.pem()).unwrap();
        std::fs::write(&key_path, ca_key.serialize_pem()).unwrap();

        let signer = FileCaSigner::from_paths(&cert_path, &key_path).expect("build FileCaSigner");

        // Build a CSR for an agent with a fresh keypair. Agents emit
        // CSRs with CN = bare machineId; the CP rewrites to canonical.
        let agent_key =
            RcgenKeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("agent keypair");
        let mut agent_params = CertificateParams::new(vec!["krach".to_string()]).unwrap();
        agent_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "krach");
        let csr = agent_params
            .serialize_request(&agent_key)
            .expect("serialize CSR");

        let now = chrono::Utc::now();
        let (cert_pem, _not_after) = issue_cert(
            &csr.pem().unwrap(),
            &signer,
            std::time::Duration::from_secs(3600),
            now,
            "fleet.lab.internal",
        )
        .expect("issue agent cert");

        // Verify the issued PEM parses as an X.509 cert + CN was
        // canonicalised to `agent-<machineId>.<suffix>` (D14).
        let parsed =
            rcgen::CertificateParams::from_ca_cert_pem(&cert_pem).expect("parse issued cert");
        let cn = parsed
            .distinguished_name
            .iter()
            .find_map(|(t, v)| {
                if matches!(t, rcgen::DnType::CommonName) {
                    Some(v.clone())
                } else {
                    None
                }
            })
            .expect("cert has CN");
        let cn_str = match cn {
            rcgen::DnValue::PrintableString(s) => s.to_string(),
            rcgen::DnValue::Utf8String(s) => s.to_string(),
            _ => panic!("unexpected CN type"),
        };
        assert_eq!(cn_str, "agent-krach.fleet.lab.internal");
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
        assert!(msg.contains("declarative-enrollment policy"), "msg = {msg}",);
    }

    #[test]
    fn rejects_when_declared_pubkey_unparseable() {
        let csr_raw = [0x42u8; 32];
        let err = validate_csr_against_fleet_host(&csr_raw, Some("ssh-rsa garbage")).unwrap_err();
        let msg = format!("{err}");
        // The error chain mentions the parse failure context.
        assert!(msg.contains("parse declared"), "msg = {msg}");
    }
}

#[cfg(test)]
mod extract_pem_tests {
    use super::*;

    #[test]
    fn accepts_single_block_key_pem() {
        // Body is opaque to the extractor - it just needs the labels.
        // PKCS#8 = rcgen / mkcert / openssl pkcs8 -topk8; SEC1 = openssl ecparam -genkey.
        let cases = [
            (
                "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
                "-----BEGIN PRIVATE KEY-----",
            ),
            (
                "-----BEGIN EC PRIVATE KEY-----\nBBBB\n-----END EC PRIVATE KEY-----\n",
                "-----BEGIN EC PRIVATE KEY-----",
            ),
        ];
        for (input, expected_label) in cases {
            let got = extract_private_key_pem_block(input).expect("single block");
            assert!(got.starts_with(expected_label), "got: {got}");
        }
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
        assert!(
            !got.contains("EC PARAMETERS"),
            "must drop the parameters block"
        );
        assert!(got.contains("MHcCAQEEIBoldKey"));
    }

    #[test]
    fn rejects_input_without_key_block() {
        // All non-key inputs must surface the same error mentioning the
        // accepted PEM labels - the operator-facing hint.
        let cases: &[(&str, &str)] = &[
            (
                "no key block (only EC PARAMETERS)",
                "-----BEGIN EC PARAMETERS-----\nBggqhkjOPQMBBw==\n-----END EC PARAMETERS-----\n",
            ),
            ("garbage (not PEM at all)", "not a pem file at all"),
            (
                "non-key block (CERTIFICATE)",
                "-----BEGIN CERTIFICATE-----\nZHVtbXk=\n-----END CERTIFICATE-----\n",
            ),
        ];
        for (label, input) in cases {
            let err = extract_private_key_pem_block(input)
                .err()
                .unwrap_or_else(|| panic!("expected error for: {label}"));
            let msg = format!("{err}");
            assert!(
                msg.contains("PRIVATE KEY"),
                "case '{label}': msg should mention accepted labels: {msg}",
            );
        }
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

    /// LOADBEARING: an actual OpenSSL-generated `prime256v1` SEC1 PEM
    /// (multi-block: EC PARAMETERS + EC PRIVATE KEY) must extract to a
    /// single-block SEC1 PEM that rcgen accepts. This is the shape lab's
    /// `fleet-ca-key.age` had - pre-fix, rcgen's `from_pem` (on the
    /// default `ring` feature) rejected it. The fix combines: extract
    /// the SEC1 block here + rcgen built with `aws_lc_rs` feature
    /// (Cargo.toml) so SEC1 is accepted natively.
    ///
    /// Key bytes are throwaway test material (no secrets) - generated
    /// with `openssl ecparam -genkey -name prime256v1` for this test.
    /// Wrapped with the `EC PARAMETERS` block to match the shape
    /// `-text` mode emits - same as lab's pre-fix `fleet-ca-key.age`.
    #[test]
    fn handles_openssl_generated_multi_block_sec1() {
        use rcgen::KeyPair;
        let openssl_pem = "\
-----BEGIN EC PARAMETERS-----
BggqhkjOPQMBBw==
-----END EC PARAMETERS-----
-----BEGIN EC PRIVATE KEY-----
MHcCAQEEIKVWabY7MNGUf1iYmLXO8Jf8Z2Dyt3wqIGzKzr+VPvvjoAoGCCqGSM49
AwEHoUQDQgAEm9EgwijVZ1xORnA9p5crCZ60IGnjUJ4LZIXzk2hlxYeiifsnGk7H
QzkM5XocGuChmeKIaGD20dCxzEIuW+HP4Q==
-----END EC PRIVATE KEY-----
";
        let extracted =
            extract_private_key_pem_block(openssl_pem).expect("multi-block SEC1 extraction");
        // Must extract just the EC PRIVATE KEY block.
        assert!(extracted.starts_with("-----BEGIN EC PRIVATE KEY-----"));
        assert!(!extracted.contains("EC PARAMETERS"));
        // And rcgen (on aws_lc_rs feature) must parse the result.
        let _key = KeyPair::from_pem(&extracted).expect(
            "rcgen on aws_lc_rs must accept SEC1 - if this fails, \
             check the rcgen feature flags in Cargo.toml",
        );
    }
}

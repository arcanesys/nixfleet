//! Sign JCS-canonical event payloads with the SSH host key. The auditor trust
//! root rotates independently from mTLS, so a leaked agent cert doesn't
//! compromise the third-party chain.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;

pub use nixfleet_proto::evidence_signing::{
    ActivationFailedSignedPayload, ClosureSignatureMismatchSignedPayload,
    ComplianceFailureSignedPayload, ManifestMismatchSignedPayload, ManifestMissingSignedPayload,
    ManifestVerifyFailedSignedPayload, RealiseFailedSignedPayload, RollbackTriggeredSignedPayload,
    RuntimeGateErrorSignedPayload, StaleTargetSignedPayload, VerifyMismatchSignedPayload,
};

pub const DEFAULT_SSH_HOST_KEY_PATH: &str = "/etc/ssh/ssh_host_ed25519_key";

pub struct EvidenceSigner {
    signing_key: SigningKey,
}

impl EvidenceSigner {
    /// `Ok(None)` when absent; `Err` on parse errors, wrong algorithm, or non-NotFound IO.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    path = %path.display(),
                    "ssh host key not found - evidence signing disabled (no auditor chain)",
                );
                return Ok(None);
            }
            Err(err) => {
                return Err(err).with_context(|| format!("read {}", path.display()));
            }
        };

        let private = ssh_key::PrivateKey::from_openssh(&raw)
            .with_context(|| format!("parse OpenSSH key at {}", path.display()))?;

        // FOOTGUN: OpenSSH stores 64 bytes (seed + pubkey); dalek wants only the 32-byte seed.
        let key_data = match private.key_data() {
            ssh_key::private::KeypairData::Ed25519(kp) => kp.private.to_bytes(),
            other => {
                anyhow::bail!(
                    "ssh host key at {} is not ed25519 (algorithm: {:?})",
                    path.display(),
                    other.algorithm()
                );
            }
        };
        let signing_key = SigningKey::from_bytes(&key_data);

        Ok(Some(Self { signing_key }))
    }

    /// base64-standard 64-byte ed25519 sig.
    pub fn sign<T: Serialize>(&self, payload: &T) -> Result<String> {
        let canonical = serde_jcs::to_vec(payload)
            .context("JCS canonicalisation of evidence payload failed")?;
        let sig = self.signing_key.sign(&canonical);
        Ok(base64::engine::general_purpose::STANDARD.encode(sig.to_bytes()))
    }
}

/// Hex SHA-256 of JCS-canonical bytes; binds evidence_snippet to its envelope.
pub fn sha256_jcs<T: Serialize>(payload: &T) -> Result<String> {
    nixfleet_canonicalize::sha256_jcs_hex(payload)
}

pub fn default_ssh_host_key_path() -> PathBuf {
    PathBuf::from(DEFAULT_SSH_HOST_KEY_PATH)
}

/// Returns `None` for both "not configured" and "configured but failed";
/// the runtime-failure path emits an `error!` so auditors can distinguish them.
pub fn try_sign<T: Serialize>(signer: &EvidenceSigner, payload: &T) -> Option<String> {
    match signer.sign(payload) {
        Ok(sig) => Some(sig),
        Err(err) => {
            tracing::error!(
                error = ?err,
                "evidence_signer.sign failed; posting unsigned event \
                 (signing was configured, runtime failure)",
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    fn write_test_key(dir: &Path) -> PathBuf {
        // Roll the seed by hand: SigningKey::generate is feature-gated.
        use ed25519_dalek::SigningKey;
        use rand::RngCore;
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let sk = SigningKey::from_bytes(&seed);
        let kp = ssh_key::private::Ed25519Keypair {
            public: ssh_key::public::Ed25519PublicKey(sk.verifying_key().to_bytes()),
            private: ssh_key::private::Ed25519PrivateKey::from_bytes(&sk.to_bytes()),
        };
        let pk = ssh_key::PrivateKey::new(ssh_key::private::KeypairData::Ed25519(kp), "test-host")
            .expect("ssh PrivateKey::new");
        let pem = pk.to_openssh(ssh_key::LineEnding::LF).expect("to_openssh");
        let path = dir.join("ssh_host_ed25519_key");
        std::fs::write(&path, pem.as_bytes()).expect("write key");
        path
    }

    #[test]
    fn load_returns_none_when_missing() {
        let result = EvidenceSigner::load(Path::new("/nonexistent/key"));
        match result {
            Ok(None) => {}
            other => panic!("expected Ok(None), got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn sign_produces_verifiable_signature() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_test_key(dir.path());
        let signer = EvidenceSigner::load(&path)
            .expect("load")
            .expect("signer present");

        let payload = ComplianceFailureSignedPayload {
            hostname: "lab",
            rollout: Some("edge-slow@abc"),
            control_id: "auditLogging",
            status: "non-compliant",
            framework_articles: &["nis2:21(b)".to_string()],
            evidence_collected_at: chrono::Utc::now(),
            evidence_snippet_sha256: "deadbeef".to_string(),
        };

        let sig_b64 = signer.sign(&payload).expect("sign");
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&sig_b64)
            .expect("base64 decode");
        let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().expect("64-byte sig");
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

        let canonical = serde_jcs::to_vec(&payload).expect("canonicalise");
        let vk = signer.signing_key.verifying_key();
        vk.verify(&canonical, &sig).expect("verify");
    }

    #[test]
    fn sign_changes_when_payload_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_test_key(dir.path());
        let signer = EvidenceSigner::load(&path)
            .expect("load")
            .expect("signer present");

        let p1 = ComplianceFailureSignedPayload {
            hostname: "lab",
            rollout: Some("edge-slow@abc"),
            control_id: "auditLogging",
            status: "non-compliant",
            framework_articles: &[],
            evidence_collected_at: chrono::Utc::now(),
            evidence_snippet_sha256: "aaa".to_string(),
        };
        let mut p2 = p1.clone();
        p2.control_id = "backupRetention";

        let s1 = signer.sign(&p1).expect("sign 1");
        let s2 = signer.sign(&p2).expect("sign 2");
        assert_ne!(s1, s2);
    }

    #[test]
    fn sha256_jcs_is_stable() {
        let v = serde_json::json!({"a": 1, "b": [2, 3]});
        let h1 = sha256_jcs(&v).unwrap();
        let h2 = sha256_jcs(&v).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn sha256_jcs_differs_on_field_change() {
        let v1 = serde_json::json!({"a": 1});
        let v2 = serde_json::json!({"a": 2});
        assert_ne!(sha256_jcs(&v1).unwrap(), sha256_jcs(&v2).unwrap());
    }
}

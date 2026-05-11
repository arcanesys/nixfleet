//! Probe-output signature verification. Pubkey is OpenSSH `ssh-ed25519 ...`
//! sourced from `hosts.<hostname>.pubkey`.

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignatureStatus {
    Verified,
    Unsigned,
    NoPubkey,
    /// Signature verification refused - active tampering.
    Mismatch,
    /// Decoding or pubkey parse failed - active tampering.
    Malformed,
    /// Non-ed25519 pubkey. Soft skip.
    WrongAlgorithm,
}

impl SignatureStatus {
    /// Counts everything except active-tampering signals; mTLS is the primary
    /// trust root, this signature is defense-in-depth.
    pub fn counts_for_gate(self) -> bool {
        !matches!(self, SignatureStatus::Mismatch | SignatureStatus::Malformed)
    }
}

/// Verify a base64 ed25519 signature over already-canonical bytes.
pub fn verify_canonical_payload(
    canonical: &[u8],
    pubkey_openssh: Option<&str>,
    signature: Option<&str>,
) -> SignatureStatus {
    let Some(sig_b64) = signature else {
        return SignatureStatus::Unsigned;
    };
    let Some(pubkey_str) = pubkey_openssh else {
        return SignatureStatus::NoPubkey;
    };

    let pubkey = match parse_ssh_ed25519_pubkey(pubkey_str) {
        Ok(Some(k)) => k,
        Ok(None) => return SignatureStatus::WrongAlgorithm,
        Err(_) => return SignatureStatus::Malformed,
    };

    let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(sig_b64) {
        Ok(b) => b,
        Err(_) => return SignatureStatus::Malformed,
    };
    let sig_arr: [u8; 64] = match sig_bytes.as_slice().try_into() {
        Ok(a) => a,
        Err(_) => return SignatureStatus::Malformed,
    };
    let sig = Signature::from_bytes(&sig_arr);

    match pubkey.verify(canonical, &sig) {
        Ok(()) => SignatureStatus::Verified,
        Err(_) => SignatureStatus::Mismatch,
    }
}

/// JCS-canonicalize `payload`, then verify.
pub fn verify_event<T: Serialize>(
    signature: Option<&str>,
    pubkey_openssh: Option<&str>,
    payload: &T,
) -> SignatureStatus {
    let canonical = match serde_jcs::to_vec(payload) {
        Ok(v) => v,
        Err(_) => return SignatureStatus::Malformed,
    };
    verify_canonical_payload(&canonical, pubkey_openssh, signature)
}

/// `Ok(Some)` ed25519, `Ok(None)` non-ed25519, `Err` parse failure.
fn parse_ssh_ed25519_pubkey(line: &str) -> anyhow::Result<Option<VerifyingKey>> {
    use anyhow::Context;
    let public = ssh_key::PublicKey::from_openssh(line.trim()).context("parse OpenSSH pubkey")?;
    match public.key_data() {
        ssh_key::public::KeyData::Ed25519(ed) => {
            let bytes: [u8; 32] = ed.0;
            let vk =
                VerifyingKey::from_bytes(&bytes).context("ed25519 verifying key from 32 bytes")?;
            Ok(Some(vk))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nixfleet_proto::evidence_signing::ComplianceFailureSignedPayload;

    /// Deterministic distinct keypairs from a seed byte.
    fn keypair_from(byte: u8) -> (ed25519_dalek::SigningKey, String) {
        let seed = [byte; 32];
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        let pubkey_bytes = sk.verifying_key().to_bytes();
        let ssh_pk = ssh_key::PublicKey::new(
            ssh_key::public::KeyData::Ed25519(ssh_key::public::Ed25519PublicKey(pubkey_bytes)),
            "test-host",
        );
        (sk, ssh_pk.to_openssh().expect("to_openssh"))
    }

    fn sample_payload() -> ComplianceFailureSignedPayload<'static> {
        ComplianceFailureSignedPayload {
            hostname: "lab",
            rollout: Some("edge-slow@abc"),
            control_id: "auditLogging",
            status: "non-compliant",
            framework_articles: &[],
            evidence_collected_at: chrono::DateTime::from_timestamp(1_000_000, 0).unwrap(),
            evidence_snippet_sha256: "deadbeef".to_string(),
        }
    }

    #[test]
    fn unsigned_when_signature_missing() {
        assert_eq!(
            verify_event(None, Some("ssh-ed25519 AAAAxxxx"), &sample_payload()),
            SignatureStatus::Unsigned
        );
    }

    #[test]
    fn no_pubkey_when_pubkey_missing() {
        assert_eq!(
            verify_event(Some("AAAA"), None, &sample_payload()),
            SignatureStatus::NoPubkey
        );
    }

    #[test]
    fn round_trip_succeeds() {
        use ed25519_dalek::Signer;
        let (sk, pubkey_str) = keypair_from(1);
        let payload = sample_payload();
        let sig = sk.sign(&serde_jcs::to_vec(&payload).unwrap());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        assert_eq!(
            verify_event(Some(&sig_b64), Some(&pubkey_str), &payload),
            SignatureStatus::Verified
        );
    }

    #[test]
    fn mismatch_on_tampered_payload() {
        use ed25519_dalek::Signer;
        let (sk, pubkey_str) = keypair_from(1);
        let payload = sample_payload();
        let sig = sk.sign(&serde_jcs::to_vec(&payload).unwrap());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        let mut tampered = sample_payload();
        tampered.control_id = "backupRetention";
        assert_eq!(
            verify_event(Some(&sig_b64), Some(&pubkey_str), &tampered),
            SignatureStatus::Mismatch
        );
    }

    #[test]
    fn mismatch_on_wrong_pubkey() {
        use ed25519_dalek::Signer;
        let (sk_signer, _) = keypair_from(1);
        let (_, pubkey_str_other) = keypair_from(2);
        let payload = sample_payload();
        let sig = sk_signer.sign(&serde_jcs::to_vec(&payload).unwrap());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        assert_eq!(
            verify_event(Some(&sig_b64), Some(&pubkey_str_other), &payload),
            SignatureStatus::Mismatch
        );
    }

    #[test]
    fn malformed_on_garbage_signature() {
        let payload = sample_payload();
        let (_, pubkey_str) = keypair_from(3);
        assert_eq!(
            verify_event(Some("!!!not-base64!!!"), Some(&pubkey_str), &payload),
            SignatureStatus::Malformed
        );
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert_eq!(
            verify_event(Some(&short), Some(&pubkey_str), &payload),
            SignatureStatus::Malformed
        );
    }

    #[test]
    fn malformed_on_garbage_pubkey() {
        let payload = sample_payload();
        let sig = base64::engine::general_purpose::STANDARD.encode([0u8; 64]);
        assert_eq!(
            verify_event(Some(&sig), Some("ssh-ed25519 garbage"), &payload),
            SignatureStatus::Malformed
        );
    }

    #[test]
    fn signature_status_gate_counting() {
        assert!(SignatureStatus::Verified.counts_for_gate());
        assert!(SignatureStatus::Unsigned.counts_for_gate());
        assert!(SignatureStatus::NoPubkey.counts_for_gate());
        assert!(SignatureStatus::WrongAlgorithm.counts_for_gate());
        assert!(!SignatureStatus::Mismatch.counts_for_gate());
        assert!(!SignatureStatus::Malformed.counts_for_gate());
    }

    #[test]
    fn bytes_level_round_trip() {
        use ed25519_dalek::Signer;
        let (sk, pubkey_str) = keypair_from(1);
        let canonical = serde_jcs::to_vec(&sample_payload()).unwrap();
        let sig = sk.sign(&canonical);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        assert_eq!(
            verify_canonical_payload(&canonical, Some(&pubkey_str), Some(&sig_b64)),
            SignatureStatus::Verified
        );
    }

    fn round_trip<T: Serialize>(payload: &T) {
        use ed25519_dalek::Signer;
        let (sk, pubkey_str) = keypair_from(7);
        let canonical = serde_jcs::to_vec(payload).unwrap();
        let sig = sk.sign(&canonical);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        assert_eq!(
            verify_event(Some(&sig_b64), Some(&pubkey_str), payload),
            SignatureStatus::Verified
        );
    }

    #[test]
    fn activation_failed_round_trip() {
        use nixfleet_proto::evidence_signing::ActivationFailedSignedPayload;
        round_trip(&ActivationFailedSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            phase: "switch-to-configuration",
            exit_code: Some(1),
            stderr_tail_sha256: "deadbeef".to_string(),
        });
    }

    #[test]
    fn realise_failed_round_trip() {
        use nixfleet_proto::evidence_signing::RealiseFailedSignedPayload;
        round_trip(&RealiseFailedSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            closure_hash: "0000000000000000000000000000000000000000-test",
            reason: "substituter 503",
        });
    }

    #[test]
    fn verify_mismatch_round_trip() {
        use nixfleet_proto::evidence_signing::VerifyMismatchSignedPayload;
        round_trip(&VerifyMismatchSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            expected: "0000000000000000000000000000000000000000-expected",
            actual: "1111111111111111111111111111111111111111-actual",
        });
    }

    #[test]
    fn rollback_triggered_round_trip() {
        use nixfleet_proto::evidence_signing::RollbackTriggeredSignedPayload;
        round_trip(&RollbackTriggeredSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            reason: "cp-410: rollout cancelled",
        });
    }

    #[test]
    fn closure_signature_mismatch_round_trip() {
        use nixfleet_proto::evidence_signing::ClosureSignatureMismatchSignedPayload;
        round_trip(&ClosureSignatureMismatchSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            closure_hash: "0000000000000000000000000000000000000000-test",
            stderr_tail_sha256: "cafebabe".to_string(),
        });
    }

    #[test]
    fn manifest_missing_round_trip() {
        use nixfleet_proto::evidence_signing::ManifestMissingSignedPayload;
        round_trip(&ManifestMissingSignedPayload {
            hostname: "lab",
            rollout: Some("a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1"),
            rollout_id: "a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1",
            reason: "GET /v1/rollouts/<id> returned 404",
        });
    }

    #[test]
    fn manifest_verify_failed_round_trip() {
        use nixfleet_proto::evidence_signing::ManifestVerifyFailedSignedPayload;
        round_trip(&ManifestVerifyFailedSignedPayload {
            hostname: "lab",
            rollout: Some("a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1"),
            rollout_id: "a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1",
            reason: "signature does not verify against ciReleaseKey",
        });
    }

    #[test]
    fn manifest_mismatch_round_trip() {
        use nixfleet_proto::evidence_signing::ManifestMismatchSignedPayload;
        round_trip(&ManifestMismatchSignedPayload {
            hostname: "lab",
            rollout: Some("a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1"),
            rollout_id: "a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1",
            reason: "(hostname, wave_index) not in manifest.host_set",
        });
    }

    #[test]
    fn stale_target_round_trip() {
        use nixfleet_proto::evidence_signing::StaleTargetSignedPayload;
        round_trip(&StaleTargetSignedPayload {
            hostname: "lab",
            rollout: Some("stable@abc"),
            closure_hash: "0000000000000000000000000000000000000000-test",
            channel_ref: "stable@abc",
            signed_at: chrono::DateTime::from_timestamp(1_000_000, 0).unwrap(),
            freshness_window_secs: 86400,
            age_secs: 3600,
        });
    }
}

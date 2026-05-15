//! Sign + smoke-verify integration. Skips build/push/git (need real
//! flake + nix daemon).

use std::process::Command;
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use ed25519_dalek::ed25519::signature::rand_core::OsRng;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_proto::{
    Channel, Compliance, FleetResolved, Host, KeySlot, Meta, TrustConfig, TrustedPubkey,
};

fn dummy_resolved() -> FleetResolved {
    let mut hosts = std::collections::HashMap::new();
    hosts.insert(
        "test-host".to_string(),
        Host {
            system: "x86_64-linux".into(),
            tags: vec![],
            channel: "stable".into(),
            closure_hash: Some("abc123-nixos-system-test-host-26.05".into()),
            pubkey: None,
            pin: None,
        },
    );
    let mut channels = std::collections::HashMap::new();
    channels.insert(
        "stable".to_string(),
        Channel {
            rollout_policy: "default".into(),
            reconcile_interval_minutes: 5,
            freshness_window: 60,
            signing_interval_minutes: 30,
            compliance: Compliance {
                frameworks: vec![],
                mode: "disabled".to_string(),
            },
        },
    );
    FleetResolved {
        schema_version: 1,
        hosts,
        channels,
        rollout_policies: Default::default(),
        waves: Default::default(),
        edges: vec![],
        channel_edges: vec![],
        disruption_budgets: vec![],
        meta: Meta {
            schema_version: 1,
            signed_at: Some(Utc::now()),
            ci_commit: Some("deadbeef".into()),
            signature_algorithm: Some("ed25519".into()),
        },
    }
}

#[test]
fn end_to_end_sign_then_verify_artifact_accepts() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());

    let resolved = dummy_resolved();
    let canonical = nixfleet_release::canonicalize_resolved(&resolved).expect("canonicalize");
    let canonical_bytes = canonical.as_bytes();
    let signature = signing_key.sign(canonical_bytes);

    let trust = TrustConfig {
        schema_version: 1,
        ci_release_key: KeySlot {
            current: Some(TrustedPubkey {
                algorithm: "ed25519".into(),
                public: pubkey_b64.clone(),
            }),
            previous: None,
            reject_before: None,
            successor: None,
            retire_at: None,
        },
        cache_keys: vec![],
        org_root_key: None,
        root_ca_pem: None,
        issuance_ca_pems: vec![],
    };
    let trusted_keys = trust.ci_release_key.active_keys();
    let parsed = nixfleet_reconciler::verify_artifact(
        canonical_bytes,
        &signature.to_bytes(),
        &trusted_keys,
        Utc::now(),
        Duration::from_secs(86400 * 365 * 10),
        None,
    )
    .expect("verify_artifact accepts real signature");

    assert_eq!(
        parsed.hosts["test-host"].closure_hash.as_deref(),
        Some("abc123-nixos-system-test-host-26.05"),
        "verified artifact carries the injected closureHash"
    );
}

#[test]
fn shell_hook_contract_invokes_sh_with_env_vars() {
    // sh hook records env + copies input -> output.
    let tmpdir = tempfile::tempdir().unwrap();
    let log = tmpdir.path().join("hook.log");
    let log_str = log.to_string_lossy();
    let cmd = format!(
        r#"echo "$NIXFLEET_INPUT" >> {log}; echo "$NIXFLEET_OUTPUT" >> {log}; cat "$NIXFLEET_INPUT" > "$NIXFLEET_OUTPUT""#,
        log = log_str,
    );

    let in_file = tmpdir.path().join("in");
    let out_file = tmpdir.path().join("out");
    std::fs::write(&in_file, b"some-canonical-bytes").unwrap();
    std::fs::write(&out_file, b"").unwrap();

    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .env("NIXFLEET_INPUT", &in_file)
        .env("NIXFLEET_OUTPUT", &out_file)
        .status()
        .unwrap();
    assert!(status.success());

    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(log_text.contains(in_file.to_str().unwrap()));
    assert!(log_text.contains(out_file.to_str().unwrap()));
    let copied = std::fs::read(&out_file).unwrap();
    assert_eq!(copied, b"some-canonical-bytes");
}

#[test]
fn inject_closure_hashes_silently_skips_unknown_hosts() {
    let mut resolved = dummy_resolved();
    let mut hashes = std::collections::BTreeMap::new();
    hashes.insert("test-host".to_string(), "real-hash".to_string());
    hashes.insert("ghost-host".to_string(), "phantom".to_string());

    nixfleet_release::inject_closure_hashes(&mut resolved, &hashes);

    assert_eq!(
        resolved.hosts["test-host"].closure_hash.as_deref(),
        Some("real-hash"),
    );
    assert!(!resolved.hosts.contains_key("ghost-host"));
}

#[test]
fn canonicalize_resolved_is_byte_stable_round_trip() {
    let resolved = dummy_resolved();
    let c1 = nixfleet_release::canonicalize_resolved(&resolved).expect("first canonicalize");
    let parsed: nixfleet_proto::FleetResolved =
        serde_json::from_str(&c1).expect("canonical bytes must parse as FleetResolved");
    let c2 = nixfleet_release::canonicalize_resolved(&parsed).expect("second canonicalize");
    assert_eq!(
        c1.as_bytes(),
        c2.as_bytes(),
        "canonicalize must be byte-stable through one round-trip",
    );
}

#[test]
fn render_commit_message_substitutes_known_placeholders() {
    let ts = chrono::DateTime::parse_from_rfc3339("2026-04-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let msg = nixfleet_release::render_commit_message(
        "release: {sha} short={sha:0:8} at {ts}",
        "abc12345deadbeef",
        ts,
    );
    assert!(msg.contains("abc12345deadbeef"), "{{sha}} expanded: {msg}");
    assert!(
        msg.contains("short=abc12345 "),
        "{{sha:0:8}} truncated to 8 chars: {msg}",
    );
    assert!(
        msg.contains("2026-04-30T12:00:00"),
        "{{ts}} expanded: {msg}"
    );
}

#[test]
fn render_commit_message_short_sha_under_8_chars_passes_through() {
    // sha < 8 chars bypasses the slice and substitutes as-is.
    let ts = chrono::DateTime::parse_from_rfc3339("2026-04-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let msg = nixfleet_release::render_commit_message("at {sha:0:8}", "HEAD", ts);
    assert_eq!(msg, "at HEAD", "short sha passes through untouched: {msg}");
}

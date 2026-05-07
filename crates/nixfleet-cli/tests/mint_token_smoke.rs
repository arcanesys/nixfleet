//! Bin-level smoke test for `nixfleet mint-token` subcommand: drives
//! the umbrella binary with a synthetic org root key + fleet.resolved
//! and confirms a parseable BootstrapToken is emitted on stdout.

use std::io::Write;
use std::process::Command;

use base64::Engine;

#[test]
fn nixfleet_mint_token_subcommand_emits_signed_token() {
    let dir = tempfile::TempDir::new().unwrap();

    // 32-byte org root key (raw bytes path; mint accepts this).
    let org_root_key = dir.path().join("org-root.bin");
    std::fs::write(&org_root_key, [0x42u8; 32]).unwrap();

    // Minimal fleet.resolved declaring the test host with a known pubkey.
    let raw_pubkey = [0x55u8; 32];
    let mut blob = Vec::new();
    blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&(raw_pubkey.len() as u32).to_be_bytes());
    blob.extend_from_slice(&raw_pubkey);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
    let openssh = format!("ssh-ed25519 {b64} test@host");
    let fleet_json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            "test-host": {
                "system": "x86_64-linux",
                "tags": [],
                "channel": "stable",
                "closureHash": null,
                "pubkey": openssh,
            }
        },
        "channels": {
            "stable": {
                "rolloutPolicy": "default",
                "reconcileIntervalMinutes": 5,
                "freshnessWindow": 60,
                "signingIntervalMinutes": 30,
                "compliance": { "frameworks": [], "mode": "disabled" },
            }
        },
        "rolloutPolicies": {
            "default": {
                "strategy": "waves",
                "waves": [],
                "healthGate": {},
                "onHealthFailure": "halt",
            }
        },
        "waves": {},
        "edges": [],
        "disruptionBudgets": [],
        "meta": {
            "schemaVersion": 1,
            "signedAt": null,
            "ciCommit": null,
            "signatureAlgorithm": null,
        }
    });
    let fleet_path = dir.path().join("fleet.resolved.json");
    let mut f = std::fs::File::create(&fleet_path).unwrap();
    f.write_all(fleet_json.to_string().as_bytes()).unwrap();

    let bin = env!("CARGO_BIN_EXE_nixfleet");
    let output = Command::new(bin)
        .args([
            "mint-token",
            "--hostname",
            "test-host",
            "--fleet-resolved",
            fleet_path.to_str().unwrap(),
            "--org-root-key",
            org_root_key.to_str().unwrap(),
        ])
        .output()
        .expect("spawn nixfleet mint-token");

    assert!(
        output.status.success(),
        "mint-token failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let token: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be valid JSON BootstrapToken");
    assert_eq!(token["claims"]["hostname"], "test-host");
    assert_eq!(token["version"], 1);
    assert!(!token["signature"].as_str().unwrap().is_empty());
}

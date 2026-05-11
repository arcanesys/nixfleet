//! Bin-level smoke test: invoke the binary with NIXFLEET_OPERATOR_FLEET_ROOT_*
//! env vars set instead of flags, confirm the env-fallback path resolves
//! and outputs land. Lib-level mint correctness is covered by
//! crates/nixfleet-cli/src/operator_cert.rs unit tests.

use std::path::PathBuf;
use std::process::Command;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use tempfile::TempDir;

fn fresh_root(dir: &TempDir) -> (PathBuf, PathBuf) {
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "Smoke Test Root");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let cert_path = dir.path().join("root.cert.pem");
    let key_path = dir.path().join("root.key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    (cert_path, key_path)
}

#[test]
fn bin_resolves_root_paths_via_env() {
    let dir = TempDir::new().unwrap();
    let (root_cert, root_key) = fresh_root(&dir);
    let output_cert = dir.path().join("operator.pem");
    let output_key = dir.path().join("operator.key");

    let bin_path = env!("CARGO_BIN_EXE_nixfleet");
    let status = Command::new(bin_path)
        .env("NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE", &root_cert)
        .env("NIXFLEET_OPERATOR_FLEET_ROOT_KEY_FILE", &root_key)
        .args([
            "mint-operator-cert",
            "--cn",
            "operator-smoke@host",
            "--output-cert",
            output_cert.to_str().unwrap(),
            "--output-key",
            output_key.to_str().unwrap(),
        ])
        .status()
        .expect("spawn nixfleet mint-operator-cert");

    assert!(status.success(), "bin must exit 0, got {status:?}");
    assert!(output_cert.exists(), "cert must be written");
    assert!(output_key.exists(), "key must be written");
}

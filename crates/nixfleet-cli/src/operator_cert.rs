//! Operator-cert mint. Offline crypto only - never opens a socket. Generates
//! an ECDSA-P-256 keypair, signs a clientAuth-EKU child with the fleet root
//! cert + key, atomic-writes both PEMs.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};

pub struct MintOperatorCertArgs {
    pub root_cert_path: PathBuf,
    pub root_key_path: PathBuf,
    pub cn: String,
    pub output_cert_path: PathBuf,
    pub output_key_path: PathBuf,
    pub validity_days: u32,
    pub overwrite: bool,
}

#[derive(Debug)]
pub struct MintOutcome {
    pub cn: String,
    pub not_after: DateTime<Utc>,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub fn mint_operator_cert(args: MintOperatorCertArgs) -> Result<MintOutcome> {
    if args.cn.is_empty() {
        bail!("operator CN is empty");
    }
    if args.validity_days == 0 {
        bail!("validity must be at least 1 day; got 0");
    }
    for path in [&args.output_cert_path, &args.output_key_path] {
        if path.exists() && !args.overwrite {
            bail!(
                "{} already exists; pass --force to overwrite",
                path.display()
            );
        }
    }

    let ca_cert_pem = std::fs::read_to_string(&args.root_cert_path)
        .with_context(|| format!("read fleet root cert {}", args.root_cert_path.display()))?;
    let ca_key_pem = std::fs::read_to_string(&args.root_key_path)
        .with_context(|| format!("read fleet root key {}", args.root_key_path.display()))?;

    let ca_key = KeyPair::from_pem(&ca_key_pem).context("parse fleet root key PEM")?;
    // Reject non-ECDSA-P-256 roots: trust chain is P-256 (issuance CA + agent
    // certs); off-algorithm roots won't chain at the CP's mTLS layer.
    let algo = ca_key.algorithm();
    if algo != &rcgen::PKCS_ECDSA_P256_SHA256 {
        bail!(
            "fleet root key must be ECDSA-P-256 (matches issuance CA chain); got {:?}",
            algo,
        );
    }
    let ca_params =
        CertificateParams::from_ca_cert_pem(&ca_cert_pem).context("parse fleet root cert PEM")?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .context("rebuild fleet root CA from PEM")?;

    let child_key = KeyPair::generate().context("generate operator keypair")?;
    let now = Utc::now();
    let not_before = now - chrono::Duration::minutes(5);
    let not_after = now + chrono::Duration::days(i64::from(args.validity_days));
    // rcgen wants `time::OffsetDateTime`; bridge through `SystemTime`.
    let not_before_sys = std::time::SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(not_before.timestamp().max(0) as u64);
    let not_after_sys = std::time::SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(not_after.timestamp().max(0) as u64);

    let mut child_params = CertificateParams::default();
    child_params
        .distinguished_name
        .push(DnType::CommonName, args.cn.clone());
    child_params
        .distinguished_name
        .push(DnType::OrganizationName, "arcanesys");
    child_params
        .distinguished_name
        .push(DnType::OrganizationalUnitName, "fleet");
    child_params.is_ca = IsCa::ExplicitNoCa;
    child_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    // CN-only is fine for clientAuth (webpki's dNSName check is server-side).
    // Operator certs never serve, so no SAN.
    child_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    child_params.not_before = not_before_sys.into();
    child_params.not_after = not_after_sys.into();

    let child_cert = child_params
        .signed_by(&child_key, &ca_cert, &ca_key)
        .context("sign operator cert with fleet root")?;

    write_atomic_with_mode(&args.output_cert_path, child_cert.pem().as_bytes(), 0o644)?;
    write_atomic_with_mode(
        &args.output_key_path,
        child_key.serialize_pem().as_bytes(),
        0o600,
    )?;

    Ok(MintOutcome {
        cn: args.cn,
        not_after,
        cert_path: args.output_cert_path,
        key_path: args.output_key_path,
    })
}

fn write_atomic_with_mode(path: &std::path::Path, body: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", rand::random::<u32>()));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)
            .with_context(|| format!("create temp {}", tmp.display()))?;
        f.write_all(body)
            .with_context(|| format!("write temp {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::write(&tmp, body).with_context(|| format!("write temp {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, KeyUsagePurpose};
    use tempfile::TempDir;

    /// Mint a fresh self-signed ECDSA-P-256 CA into `dir` and return paths.
    fn fresh_root_pki(dir: &TempDir) -> (PathBuf, PathBuf) {
        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "Test Fleet Root CA");
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

    fn mint_args(dir: &TempDir, root_cert: PathBuf, root_key: PathBuf) -> MintOperatorCertArgs {
        MintOperatorCertArgs {
            root_cert_path: root_cert,
            root_key_path: root_key,
            cn: "operator-test@host".into(),
            output_cert_path: dir.path().join("operator.pem"),
            output_key_path: dir.path().join("operator.key"),
            validity_days: 365,
            overwrite: false,
        }
    }

    #[test]
    fn mints_cert_signed_by_provided_root() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let outcome = mint_operator_cert(mint_args(&dir, root_cert.clone(), root_key)).unwrap();
        assert_eq!(outcome.cn, "operator-test@host");
        assert!(outcome.cert_path.exists());
        assert!(outcome.key_path.exists());

        let leaf_pem = std::fs::read_to_string(&outcome.cert_path).unwrap();
        let root_pem = std::fs::read_to_string(&root_cert).unwrap();
        let (_, leaf) = x509_parser::pem::parse_x509_pem(leaf_pem.as_bytes()).unwrap();
        let (_, root) = x509_parser::pem::parse_x509_pem(root_pem.as_bytes()).unwrap();
        let leaf_cert = leaf.parse_x509().unwrap();
        let root_cert_parsed = root.parse_x509().unwrap();
        assert_eq!(
            leaf_cert.issuer().to_string(),
            root_cert_parsed.subject().to_string(),
            "leaf issuer should match root subject",
        );
        leaf_cert
            .verify_signature(Some(root_cert_parsed.public_key()))
            .expect("leaf signature must verify against root pubkey");
    }

    #[test]
    fn output_cert_has_correct_cn_and_eku() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let outcome = mint_operator_cert(mint_args(&dir, root_cert, root_key)).unwrap();

        let pem = std::fs::read_to_string(&outcome.cert_path).unwrap();
        let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
        let cert = parsed.parse_x509().unwrap();
        assert!(
            cert.subject().to_string().contains("CN=operator-test@host"),
            "subject must include CN: got {}",
            cert.subject(),
        );
        assert!(cert.subject().to_string().contains("O=arcanesys"));
        let eku = cert
            .extended_key_usage()
            .unwrap()
            .expect("EKU extension")
            .value;
        assert!(eku.client_auth, "EKU must include clientAuth");
        let bc = cert
            .basic_constraints()
            .unwrap()
            .expect("BC extension")
            .value;
        assert!(!bc.ca, "BasicConstraints.cA must be false");
    }

    #[test]
    fn validity_window_matches_days() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let mut args = mint_args(&dir, root_cert, root_key);
        args.validity_days = 30;
        let now = Utc::now();
        let outcome = mint_operator_cert(args).unwrap();
        let delta = outcome.not_after - now;
        assert!(
            delta.num_days() >= 29 && delta.num_days() <= 30,
            "expected ~30 days, got {} days",
            delta.num_days(),
        );
    }

    #[test]
    fn refuses_overwrite_without_force() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let args = mint_args(&dir, root_cert.clone(), root_key.clone());
        let cert_path = args.output_cert_path.clone();
        mint_operator_cert(args).unwrap();

        let original = std::fs::read(&cert_path).unwrap();
        let args2 = mint_args(&dir, root_cert, root_key);
        let err = mint_operator_cert(args2).unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "expected refusal, got: {err}",
        );
        let after = std::fs::read(&cert_path).unwrap();
        assert_eq!(original, after, "output must be untouched on refusal");
    }

    #[test]
    fn overwrites_with_force() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let args1 = mint_args(&dir, root_cert.clone(), root_key.clone());
        mint_operator_cert(args1).unwrap();
        let mut args2 = mint_args(&dir, root_cert, root_key);
        args2.cn = "operator-replaced@host".into();
        args2.overwrite = true;
        mint_operator_cert(args2).unwrap();

        let pem = std::fs::read_to_string(dir.path().join("operator.pem")).unwrap();
        let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap();
        let cert = parsed.parse_x509().unwrap();
        assert!(
            cert.subject()
                .to_string()
                .contains("CN=operator-replaced@host"),
            "force overwrite should produce new CN: got {}",
            cert.subject(),
        );
    }

    #[test]
    fn rejects_non_ecdsa_root_key() {
        let dir = TempDir::new().unwrap();
        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "Ed25519 Root");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ed_key = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let cert = params.self_signed(&ed_key).unwrap();
        let cert_path = dir.path().join("root.cert.pem");
        let key_path = dir.path().join("root.key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, ed_key.serialize_pem()).unwrap();

        let err = mint_operator_cert(mint_args(&dir, cert_path, key_path)).unwrap_err();
        assert!(
            err.to_string().contains("ECDSA-P-256"),
            "expected ECDSA-P-256 rejection, got: {err}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_modes_are_0644_cert_0600_key_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let outcome = mint_operator_cert(mint_args(&dir, root_cert, root_key)).unwrap();
        let cert_mode = std::fs::metadata(&outcome.cert_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let key_mode = std::fs::metadata(&outcome.key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(cert_mode, 0o644, "cert mode");
        assert_eq!(key_mode, 0o600, "key mode");
    }

    /// Smoke check on PEM well-formedness. Cryptographic pairing is covered
    /// transitively by `mints_cert_signed_by_provided_root`.
    #[test]
    fn output_pems_decode_without_error() {
        let dir = TempDir::new().unwrap();
        let (root_cert, root_key) = fresh_root_pki(&dir);
        let outcome = mint_operator_cert(mint_args(&dir, root_cert, root_key)).unwrap();

        let cert_pem = std::fs::read_to_string(&outcome.cert_path).unwrap();
        let key_pem = std::fs::read_to_string(&outcome.key_path).unwrap();
        KeyPair::from_pem(&key_pem).expect("operator key parses");
        CertificateParams::from_ca_cert_pem(&cert_pem).expect("operator cert parses");
    }
}

//! CLI config precedence and `${HOSTNAME}` fallback.
//!
//! Tests call `cli::config::resolve` and `cli::config::expand_env_vars`
//! directly via the lib target. No CP is involved.
//!
//! Precedence (high → low):
//! CLI flag → `NIXFLEET_*` env → credentials file → `.nixfleet.toml`.

use nixfleet::config::{self, ConfigFile, CredentialsFile, ResolvedConfig};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Process-wide lock serializing every test that touches `NIXFLEET_*` env
/// vars. cargo test runs tests in parallel by default; std::env mutations
/// are global so two parallel tests reading/writing the same vars race.
/// Every env-touching test in this file calls `env_lock()` first.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
}

/// Clear every NIXFLEET_* env var that `resolve()` reads. Used at the
/// start of every env-sensitive test so the test starts from a known
/// blank-env baseline regardless of leakage from sibling tests or the
/// developer's outer shell.
fn clear_nixfleet_env() {
    for k in [
        "NIXFLEET_CONTROL_PLANE_URL",
        "NIXFLEET_API_KEY",
        "NIXFLEET_CA_CERT",
        "NIXFLEET_CLIENT_CERT",
        "NIXFLEET_CLIENT_KEY",
    ] {
        std::env::remove_var(k);
    }
}

fn empty_credentials() -> CredentialsFile {
    CredentialsFile {
        entries: HashMap::new(),
    }
}

/// Parse a TOML string into `ConfigFile`, matching how `load_config_file`
/// would load the on-disk file. Avoids touching the filesystem.
fn parse_config(toml: &str) -> ConfigFile {
    toml::from_str(toml).expect("parse test config toml")
}

/// I2 (partial) — CLI args override credentials override file.
///
/// Env-var precedence is asserted separately below in
/// `i2_env_var_precedence_overrides_credentials`.
#[test]
fn i2_cli_overrides_credentials_overrides_file() {
    let _guard = env_lock();
    clear_nixfleet_env();

    let cfg = parse_config(
        r#"
[control-plane]
url = "https://file.example"
"#,
    );

    // File-only: URL comes from the toml.
    let file_only: ResolvedConfig = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &empty_credentials(),
        config::CliOverrides {
            cp_url: "http://localhost:8080", // sentinel = unset
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        file_only.control_plane_url.as_deref(),
        Some("https://file.example")
    );

    // Credentials layer: the api_key for the file URL comes from credentials.
    let mut creds = empty_credentials();
    creds.entries.insert(
        "https://file.example".to_string(),
        nixfleet::config::CredentialEntry {
            api_key: Some("nfk-from-creds".to_string()),
        },
    );
    let with_creds = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(with_creds.api_key.as_deref(), Some("nfk-from-creds"));
    assert_eq!(
        with_creds.control_plane_url.as_deref(),
        Some("https://file.example"),
        "credentials layer must NOT change the URL"
    );

    // CLI override: cp URL from CLI wins.
    let cli = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "https://cli.example",
            api_key: "nfk-cli-key",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        cli.control_plane_url.as_deref(),
        Some("https://cli.example"),
        "CLI --control-plane-url must override file"
    );
    assert_eq!(
        cli.api_key.as_deref(),
        Some("nfk-cli-key"),
        "CLI --api-key must override credentials"
    );

    // Negative: the file URL is NOT the final value when CLI is set.
    assert_ne!(
        cli.control_plane_url.as_deref(),
        Some("https://file.example")
    );
}

/// Env-var layer — env vars override credentials but lose to CLI args.
///
/// Asserts the layering: file → credentials → env → CLI flag.
/// `NIXFLEET_API_KEY` must override the credentials-file api_key but
/// be overridden by `cli_api_key`. Serialized via `env_lock()` because
/// std::env mutations are process-wide.
#[test]
fn i2_env_var_precedence_overrides_credentials() {
    let _guard = env_lock();
    clear_nixfleet_env();

    let cfg = parse_config(
        r#"
[control-plane]
url = "https://file.example"
"#,
    );

    let mut creds = empty_credentials();
    creds.entries.insert(
        "https://file.example".to_string(),
        nixfleet::config::CredentialEntry {
            api_key: Some("nfk-from-creds".to_string()),
        },
    );

    // Env var set, CLI args empty → env wins over credentials.
    std::env::set_var("NIXFLEET_API_KEY", "nfk-from-env");
    let env_only = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        env_only.api_key.as_deref(),
        Some("nfk-from-env"),
        "env var must override credentials"
    );

    // Env var set, CLI arg also set → CLI wins.
    let cli_wins = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            api_key: "nfk-from-cli",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        cli_wins.api_key.as_deref(),
        Some("nfk-from-cli"),
        "CLI arg must override env var"
    );

    // NIXFLEET_CONTROL_PLANE_URL also overrides the file URL when set,
    // and re-binds credentials to the new URL.
    std::env::set_var("NIXFLEET_CONTROL_PLANE_URL", "https://env.example");
    creds.entries.insert(
        "https://env.example".to_string(),
        nixfleet::config::CredentialEntry {
            api_key: Some("nfk-env-url-creds".to_string()),
        },
    );
    std::env::remove_var("NIXFLEET_API_KEY");
    let env_url = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        env_url.control_plane_url.as_deref(),
        Some("https://env.example"),
        "NIXFLEET_CONTROL_PLANE_URL must override file URL"
    );
    assert_eq!(
        env_url.api_key.as_deref(),
        Some("nfk-env-url-creds"),
        "credentials must be re-checked against the env-supplied URL"
    );

    // NIXFLEET_CA_CERT layer — env wins over (no file value), loses to CLI.
    std::env::set_var("NIXFLEET_CA_CERT", "/run/env-ca.pem");
    let env_ca = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(env_ca.ca_cert.as_deref(), Some("/run/env-ca.pem"));
    let cli_ca = config::resolve(
        Some(&cfg),
        Some(Path::new(".")),
        &creds,
        config::CliOverrides {
            cp_url: "http://localhost:8080",
            ca_cert: "/run/cli-ca.pem",
            ..config::CliOverrides::default()
        },
    );
    assert_eq!(
        cli_ca.ca_cert.as_deref(),
        Some("/run/cli-ca.pem"),
        "CLI --ca-cert must override NIXFLEET_CA_CERT"
    );

    // Cleanup — drop the lock guard restores the env, but we still
    // clear NIXFLEET_* so a sibling test running RIGHT after us (after
    // the lock releases) does not see leakage.
    clear_nixfleet_env();
}

/// I3 — `${HOSTNAME}` expansion falls back to `gethostname()` when the
/// env var is unset.
#[test]
fn i3_hostname_fallback_uses_gethostname_when_env_unset() {
    let saved = std::env::var("HOSTNAME").ok();
    std::env::remove_var("HOSTNAME");

    let expanded = config::expand_env_vars("/run/agenix/agent-${HOSTNAME}-cert");
    assert!(
        !expanded.contains("${HOSTNAME}"),
        "${{HOSTNAME}} must be replaced via gethostname() fallback; got {expanded}"
    );
    assert!(expanded.starts_with("/run/agenix/agent-"));
    assert!(expanded.ends_with("-cert"));
    assert!(
        expanded.len() > "/run/agenix/agent--cert".len(),
        "hostname slot must be non-empty"
    );

    // Positive control: when HOSTNAME IS set in the env, that value wins.
    std::env::set_var("HOSTNAME", "test-host-xyz");
    let with_env = config::expand_env_vars("/run/agenix/agent-${HOSTNAME}-cert");
    assert_eq!(with_env, "/run/agenix/agent-test-host-xyz-cert");

    // Negative: a variable the test did not set stays literal (expand_env_vars
    // does not invent values for unknown vars).
    let literal = config::expand_env_vars("/x/${NONEXISTENT_VAR_12345}/y");
    assert!(
        literal.contains("${NONEXISTENT_VAR_12345}") || literal == "/x//y",
        "expected unknown var to stay literal or resolve to empty; got {literal}"
    );

    std::env::remove_var("HOSTNAME");
    if let Some(v) = saved {
        std::env::set_var("HOSTNAME", v);
    }
}

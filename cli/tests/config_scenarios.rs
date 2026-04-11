//! I2 (partial), I3 — config precedence and ${HOSTNAME} fallback.
//!
//! Tests call `cli::config::resolve` and `cli::config::expand_env_vars`
//! directly via the lib target. No CP is involved.
//!
//! Env-var precedence (NIXFLEET_* env vars overriding the config file)
//! is documented in CLAUDE.md but NOT implemented in `resolve`. The
//! ignored test below records the gap until Phase 4.

use nixfleet::config::{self, ConfigFile, CredentialsFile, ResolvedConfig};
use std::collections::HashMap;
use std::path::Path;

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
/// Env-var precedence is asserted separately (and ignored) below.
#[test]
fn i2_cli_overrides_credentials_overrides_file() {
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
        "http://localhost:8080", // sentinel = unset
        "",
        "",
        "",
        "",
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
        "http://localhost:8080",
        "",
        "",
        "",
        "",
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
        "https://cli.example",
        "nfk-cli-key",
        "",
        "",
        "",
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

/// I2 (deferred) — env-var precedence between CLI and credentials.
///
/// The PR #30 `resolve` function does not read any `NIXFLEET_*` env vars.
/// CLAUDE.md documents `NIXFLEET_API_KEY`, `NIXFLEET_CA_CERT`, etc. as
/// supported, but the only actual effect of those env vars today is that
/// the user can manually reference them via `${NIXFLEET_API_KEY}` in the
/// toml file — there is no direct env → ResolvedConfig path. Unblock in
/// Phase 4 by adding an env layer between credentials and CLI args.
#[test]
#[ignore = "env var precedence not implemented — TODO.md Phase 4 gap"]
fn i2_env_var_precedence_deferred() {
    // Re-enable by removing #[ignore] once `resolve` grows an env layer.
    // Expected assertion: setting NIXFLEET_API_KEY must override the
    // credentials-file api_key but be overridden by `cli_api_key`.
    unreachable!("deferred; see TODO.md Phase 4");
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

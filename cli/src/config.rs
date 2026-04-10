use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Parsed `.nixfleet.toml` config.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFile {
    #[serde(default)]
    pub control_plane: Option<ControlPlaneConfig>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
    #[serde(default)]
    pub deploy: Option<DeployConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ControlPlaneConfig {
    pub url: Option<String>,
    pub ca_cert: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TlsConfig {
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CacheConfig {
    pub url: Option<String>,
    pub push_to: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DeployConfig {
    pub strategy: Option<String>,
    pub health_timeout: Option<u64>,
    pub failure_threshold: Option<String>,
    pub on_failure: Option<String>,
}

/// Parsed `~/.config/nixfleet/credentials.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct CredentialsFile {
    #[serde(flatten)]
    pub entries: HashMap<String, CredentialEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CredentialEntry {
    pub api_key: Option<String>,
}

/// Resolved config after merging all sources.
#[derive(Debug, Default)]
pub struct ResolvedConfig {
    pub control_plane_url: Option<String>,
    pub api_key: Option<String>,
    pub ca_cert: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
    pub cache_url: Option<String>,
    pub push_to: Option<String>,
    pub strategy: Option<String>,
    pub health_timeout: Option<u64>,
    pub failure_threshold: Option<String>,
    pub on_failure: Option<String>,
}

/// Resolve a well-known variable by name. Falls back to env var lookup.
/// Handles HOSTNAME and HOST specially via gethostname() since they are
/// often shell builtins not exported to the environment.
fn resolve_var(name: &str) -> String {
    // HOSTNAME and HOST: use gethostname syscall as fallback
    if name == "HOSTNAME" || name == "HOST" {
        if let Ok(val) = std::env::var(name) {
            return val;
        }
        // Fallback: gethostname syscall
        if let Ok(hostname) = hostname::get() {
            return hostname.to_string_lossy().to_string();
        }
        return String::new();
    }
    std::env::var(name).unwrap_or_default()
}

/// Expand environment variables in a string: `${VAR}` → value of $VAR.
/// `${HOSTNAME}` and `${HOST}` fall back to gethostname() if not in env.
fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = resolve_var(var_name);
            result = format!("{}{}{}", &result[..start], value, &result[start + end + 1..]);
        } else {
            break;
        }
    }
    result
}

/// Resolve a path relative to the config file's directory.
fn resolve_path(path: &str, config_dir: &Path) -> String {
    let expanded = expand_env_vars(path);
    if expanded.starts_with('/') {
        expanded
    } else {
        config_dir.join(&expanded).to_string_lossy().to_string()
    }
}

/// Find `.nixfleet.toml` by walking up from `start_dir`.
pub fn find_config_file(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(".nixfleet.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Load and parse `.nixfleet.toml` from the given path.
pub fn load_config_file(path: &Path) -> Result<ConfigFile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Load credentials from `~/.config/nixfleet/credentials.toml`.
pub fn load_credentials() -> Result<CredentialsFile> {
    let path = credentials_path();
    if !path.is_file() {
        return Ok(CredentialsFile::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Path to the credentials file.
pub fn credentials_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("nixfleet")
        .join("credentials.toml")
}

/// Save an API key to the credentials file.
pub fn save_api_key(cp_url: &str, api_key: &str) -> Result<()> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Load existing credentials or create new
    let mut creds: HashMap<String, HashMap<String, String>> = if path.is_file() {
        let content = std::fs::read_to_string(&path)?;
        toml::from_str(&content).unwrap_or_default()
    } else {
        HashMap::new()
    };

    // Insert/update entry for this CP URL
    let entry = creds.entry(cp_url.to_string()).or_default();
    entry.insert("api-key".to_string(), api_key.to_string());

    let content = toml::to_string_pretty(&creds)
        .context("failed to serialize credentials")?;

    std::fs::write(&path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    // Set permissions to 600 (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Write a `.nixfleet.toml` config file.
pub fn write_config_file(
    path: &Path,
    cp_url: &str,
    ca_cert: Option<&str>,
    client_cert: Option<&str>,
    client_key: Option<&str>,
    cache_url: Option<&str>,
    push_to: Option<&str>,
) -> Result<()> {
    let mut content = String::new();

    content.push_str("[control-plane]\n");
    content.push_str(&format!("url = \"{}\"\n", cp_url));
    if let Some(ca) = ca_cert {
        content.push_str(&format!("ca-cert = \"{}\"\n", ca));
    }

    if client_cert.is_some() || client_key.is_some() {
        content.push_str("\n[tls]\n");
        if let Some(cert) = client_cert {
            content.push_str(&format!("client-cert = \"{}\"\n", cert));
        }
        if let Some(key) = client_key {
            content.push_str(&format!("client-key = \"{}\"\n", key));
        }
    }

    if cache_url.is_some() || push_to.is_some() {
        content.push_str("\n[cache]\n");
        if let Some(url) = cache_url {
            content.push_str(&format!("url = \"{}\"\n", url));
        }
        if let Some(pt) = push_to {
            content.push_str(&format!("push-to = \"{}\"\n", pt));
        }
    }

    std::fs::write(path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

/// Resolve config from all sources: config file → credentials → env vars → CLI args.
/// CLI args with default values (empty strings, "http://localhost:8080") are treated as unset.
// CRUD function arguments map directly to table columns; refactoring is busywork
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    config_file: Option<&ConfigFile>,
    config_dir: Option<&Path>,
    credentials: &CredentialsFile,
    cli_cp_url: &str,
    cli_api_key: &str,
    cli_ca_cert: &str,
    cli_client_cert: &str,
    cli_client_key: &str,
) -> ResolvedConfig {
    let mut resolved = ResolvedConfig::default();

    // Layer 1: config file
    if let Some(cfg) = config_file {
        let dir = config_dir.unwrap_or_else(|| Path::new("."));

        if let Some(ref cp) = cfg.control_plane {
            resolved.control_plane_url = cp.url.clone();
            resolved.ca_cert = cp.ca_cert.as_deref().map(|p| resolve_path(p, dir));
        }
        if let Some(ref tls) = cfg.tls {
            resolved.client_cert = tls.client_cert.as_deref().map(expand_env_vars);
            resolved.client_key = tls.client_key.as_deref().map(expand_env_vars);
        }
        if let Some(ref cache) = cfg.cache {
            resolved.cache_url = cache.url.clone();
            resolved.push_to = cache.push_to.clone();
        }
        if let Some(ref deploy) = cfg.deploy {
            resolved.strategy = deploy.strategy.clone();
            resolved.health_timeout = deploy.health_timeout;
            resolved.failure_threshold = deploy.failure_threshold.clone();
            resolved.on_failure = deploy.on_failure.clone();
        }
    }

    // Layer 2: credentials (keyed by CP URL)
    if let Some(ref cp_url) = resolved.control_plane_url {
        if let Some(entry) = credentials.entries.get(cp_url) {
            if entry.api_key.is_some() {
                resolved.api_key = entry.api_key.clone();
            }
        }
    }

    // Layer 3: CLI args (override if non-default)
    if cli_cp_url != "http://localhost:8080" && !cli_cp_url.is_empty() {
        resolved.control_plane_url = Some(cli_cp_url.to_string());
        // Re-check credentials for the new URL
        if let Some(entry) = credentials.entries.get(cli_cp_url) {
            if entry.api_key.is_some() {
                resolved.api_key = entry.api_key.clone();
            }
        }
    }
    if !cli_api_key.is_empty() {
        resolved.api_key = Some(cli_api_key.to_string());
    }
    if !cli_ca_cert.is_empty() {
        resolved.ca_cert = Some(cli_ca_cert.to_string());
    }
    if !cli_client_cert.is_empty() {
        resolved.client_cert = Some(cli_client_cert.to_string());
    }
    if !cli_client_key.is_empty() {
        resolved.client_key = Some(cli_client_key.to_string());
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars() {
        std::env::set_var("TEST_NIXFLEET_HOST", "myhost");
        assert_eq!(
            expand_env_vars("/run/agenix/agent-${TEST_NIXFLEET_HOST}-cert"),
            "/run/agenix/agent-myhost-cert"
        );
        std::env::remove_var("TEST_NIXFLEET_HOST");
    }

    #[test]
    fn test_expand_env_vars_missing() {
        assert_eq!(
            expand_env_vars("/path/${NONEXISTENT_VAR_12345}/file"),
            "/path//file"
        );
    }

    #[test]
    fn test_expand_env_vars_no_vars() {
        assert_eq!(expand_env_vars("/simple/path"), "/simple/path");
    }

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("/absolute/path", Path::new("/config/dir"));
        assert_eq!(result, "/absolute/path");
    }

    #[test]
    fn test_resolve_path_relative() {
        let result = resolve_path("certs/ca.pem", Path::new("/home/user/fleet"));
        assert_eq!(result, "/home/user/fleet/certs/ca.pem");
    }

    #[test]
    fn test_parse_config_file() {
        let toml_str = r#"
[control-plane]
url = "https://lab:8080"
ca-cert = "modules/_config/fleet-ca.pem"

[tls]
client-cert = "/run/agenix/agent-${HOSTNAME}-cert"
client-key = "/run/agenix/agent-${HOSTNAME}-key"

[cache]
url = "http://lab:5000"
push-to = "ssh://root@lab"

[deploy]
strategy = "staged"
health-timeout = 300
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let cp = config.control_plane.unwrap();
        assert_eq!(cp.url, Some("https://lab:8080".to_string()));
        assert_eq!(cp.ca_cert, Some("modules/_config/fleet-ca.pem".to_string()));
        let tls = config.tls.unwrap();
        assert_eq!(tls.client_cert, Some("/run/agenix/agent-${HOSTNAME}-cert".to_string()));
        let cache = config.cache.unwrap();
        assert_eq!(cache.url, Some("http://lab:5000".to_string()));
        assert_eq!(cache.push_to, Some("ssh://root@lab".to_string()));
        let deploy = config.deploy.unwrap();
        assert_eq!(deploy.strategy, Some("staged".to_string()));
        assert_eq!(deploy.health_timeout, Some(300));
    }

    #[test]
    fn test_parse_credentials_file() {
        let toml_str = r#"
["https://lab:8080"]
api-key = "nfk-abc123"

["https://prod:8080"]
api-key = "nfk-def456"
"#;
        let creds: CredentialsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            creds.entries.get("https://lab:8080").unwrap().api_key,
            Some("nfk-abc123".to_string())
        );
        assert_eq!(
            creds.entries.get("https://prod:8080").unwrap().api_key,
            Some("nfk-def456".to_string())
        );
    }

    #[test]
    fn test_resolve_cli_overrides_config() {
        let toml_str = r#"
[control-plane]
url = "https://lab:8080"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let creds = CredentialsFile::default();
        let resolved = resolve(
            Some(&config),
            Some(Path::new("/fleet")),
            &creds,
            "https://prod:8080",  // CLI override
            "",
            "",
            "",
            "",
        );
        assert_eq!(resolved.control_plane_url, Some("https://prod:8080".to_string()));
    }

    #[test]
    fn test_resolve_default_cli_values_dont_override() {
        let toml_str = r#"
[control-plane]
url = "https://lab:8080"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let creds = CredentialsFile::default();
        let resolved = resolve(
            Some(&config),
            Some(Path::new("/fleet")),
            &creds,
            "http://localhost:8080",  // clap default — should not override
            "",
            "",
            "",
            "",
        );
        assert_eq!(resolved.control_plane_url, Some("https://lab:8080".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn test_save_api_key_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        save_api_key("https://test.example.com", "nfk-testkey").unwrap();

        let path = credentials_path();
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials file must be 0o600, got {mode:o}");
    }
}

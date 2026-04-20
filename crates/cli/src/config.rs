use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub hook: Option<CacheHookConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CacheHookConfig {
    pub url: Option<String>,
    pub push_cmd: Option<String>,
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
    pub hook_url: Option<String>,
    pub hook_push_cmd: Option<String>,
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
pub fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = resolve_var(var_name);
            result = format!(
                "{}{}{}",
                &result[..start],
                value,
                &result[start + end + 1..]
            );
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
    toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

/// Load credentials from `~/.config/nixfleet/credentials.toml`.
pub fn load_credentials() -> Result<CredentialsFile> {
    let path = credentials_path();
    if !path.is_file() {
        return Ok(CredentialsFile::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

/// Path to the credentials file.
///
/// Prefers `$XDG_CONFIG_HOME` (via `dirs::config_dir`). Falls back to
/// `$HOME/.config` when XDG is not set, and finally to the current
/// working directory - the literal string `"~/.config"` is never
/// produced, because a tilde is not expanded by the filesystem API
/// and would materialize a directory named `~` on disk.
pub fn credentials_path() -> PathBuf {
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("nixfleet").join("credentials.toml")
}

/// Save an API key to the credentials file.
pub fn save_api_key(cp_url: &str, api_key: &str) -> Result<()> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Load existing credentials or create new. A corrupt credentials
    // file previously vanished silently via `unwrap_or_default`,
    // leaving the operator with no clue why their saved key no longer
    // existed. Surface the parse error instead.
    let mut creds: HashMap<String, HashMap<String, String>> = if path.is_file() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse existing {}", path.display()))?
    } else {
        HashMap::new()
    };

    // Insert/update entry for this CP URL
    let entry = creds.entry(cp_url.to_string()).or_default();
    entry.insert("api-key".to_string(), api_key.to_string());

    let content = toml::to_string_pretty(&creds).context("failed to serialize credentials")?;

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

/// Shape of the on-disk `.nixfleet.toml` produced by [`write_config_file`].
/// Mirrors [`ConfigFile`] but with owned strings so it can be serialized
/// without lifetime plumbing.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableConfigFile {
    #[serde(skip_serializing_if = "Option::is_none", rename = "control-plane")]
    control_plane: Option<WritableControlPlane>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tls: Option<WritableTls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache: Option<WritableCache>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deploy: Option<WritableDeploy>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableControlPlane {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ca_cert: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableTls {
    #[serde(skip_serializing_if = "Option::is_none")]
    client_cert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_key: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableCache {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    push_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook: Option<WritableCacheHook>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableCacheHook {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    push_cmd: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
struct WritableDeploy {
    #[serde(skip_serializing_if = "Option::is_none")]
    strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_failure: Option<String>,
}

/// Write a `.nixfleet.toml` config file.
///
/// Uses the `toml` crate's serializer so values containing quotes,
/// backslashes, or other special characters are escaped correctly.
/// Previously this was hand-rolled via `format!("url = \"{}\"\n", ...)`
/// which silently produced broken TOML on unusual input.
#[allow(clippy::too_many_arguments)]
pub fn write_config_file(
    path: &Path,
    cp_url: &str,
    ca_cert: Option<&str>,
    client_cert: Option<&str>,
    client_key: Option<&str>,
    cache_url: Option<&str>,
    push_to: Option<&str>,
    hook_url: Option<&str>,
    hook_push_cmd: Option<&str>,
    strategy: Option<&str>,
    on_failure: Option<&str>,
) -> Result<()> {
    let file = WritableConfigFile {
        control_plane: Some(WritableControlPlane {
            url: cp_url.to_string(),
            ca_cert: ca_cert.map(str::to_string),
        }),
        tls: if client_cert.is_some() || client_key.is_some() {
            Some(WritableTls {
                client_cert: client_cert.map(str::to_string),
                client_key: client_key.map(str::to_string),
            })
        } else {
            None
        },
        cache: if cache_url.is_some()
            || push_to.is_some()
            || hook_url.is_some()
            || hook_push_cmd.is_some()
        {
            Some(WritableCache {
                url: cache_url.map(str::to_string),
                push_to: push_to.map(str::to_string),
                hook: if hook_url.is_some() || hook_push_cmd.is_some() {
                    Some(WritableCacheHook {
                        url: hook_url.map(str::to_string),
                        push_cmd: hook_push_cmd.map(str::to_string),
                    })
                } else {
                    None
                },
            })
        } else {
            None
        },
        deploy: if strategy.is_some() || on_failure.is_some() {
            Some(WritableDeploy {
                strategy: strategy.map(str::to_string),
                on_failure: on_failure.map(str::to_string),
            })
        } else {
            None
        },
    };

    let content = toml::to_string_pretty(&file).context("failed to serialize .nixfleet.toml")?;

    std::fs::write(path, &content)
        .with_context(|| format!("failed to write {}", path.display()))?;

    Ok(())
}

/// CLI-argument overrides passed to [`resolve`]. Bundled into a struct so
/// the function signature does not grow with each new CLI flag - every
/// field is the raw post-clap value (empty string means "not set").
#[derive(Debug, Default, Clone, Copy)]
pub struct CliOverrides<'a> {
    pub cp_url: &'a str,
    pub api_key: &'a str,
    pub ca_cert: &'a str,
    pub client_cert: &'a str,
    pub client_key: &'a str,
}

/// Resolve config from all sources (low precedence → high):
///   1. config file (`.nixfleet.toml`)
///   2. credentials file (`~/.config/nixfleet/credentials.toml`)
///   3. environment variables (`NIXFLEET_*`)
///   4. CLI flags
///
/// CLI args with default values (empty strings, "http://localhost:8080") are
/// treated as unset.
pub fn resolve(
    config_file: Option<&ConfigFile>,
    config_dir: Option<&Path>,
    credentials: &CredentialsFile,
    cli: CliOverrides<'_>,
) -> ResolvedConfig {
    let CliOverrides {
        cp_url: cli_cp_url,
        api_key: cli_api_key,
        ca_cert: cli_ca_cert,
        client_cert: cli_client_cert,
        client_key: cli_client_key,
    } = cli;
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
            if let Some(ref hook) = cache.hook {
                resolved.hook_url = hook.url.clone();
                resolved.hook_push_cmd = hook.push_cmd.clone();
            }
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

    // Layer 3: environment variables (override credentials, lose to CLI).
    // Each var is treated as unset if absent or empty so an exported but
    // empty NIXFLEET_FOO does not silently clear a credentials value.
    if let Ok(v) = std::env::var("NIXFLEET_CONTROL_PLANE_URL") {
        if !v.is_empty() {
            resolved.control_plane_url = Some(v.clone());
            // Re-check credentials for the new URL.
            if let Some(entry) = credentials.entries.get(&v) {
                if entry.api_key.is_some() {
                    resolved.api_key = entry.api_key.clone();
                }
            }
        }
    }
    if let Ok(v) = std::env::var("NIXFLEET_API_KEY") {
        if !v.is_empty() {
            resolved.api_key = Some(v);
        }
    }
    if let Ok(v) = std::env::var("NIXFLEET_CA_CERT") {
        if !v.is_empty() {
            resolved.ca_cert = Some(v);
        }
    }
    if let Ok(v) = std::env::var("NIXFLEET_CLIENT_CERT") {
        if !v.is_empty() {
            resolved.client_cert = Some(v);
        }
    }
    if let Ok(v) = std::env::var("NIXFLEET_CLIENT_KEY") {
        if !v.is_empty() {
            resolved.client_key = Some(v);
        }
    }

    // Layer 4: CLI args (override if non-default)
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
url = "https://cp-01:8080"
ca-cert = "modules/_config/fleet-ca.pem"

[tls]
client-cert = "/run/agenix/agent-${HOSTNAME}-cert"
client-key = "/run/agenix/agent-${HOSTNAME}-key"

[cache]
url = "http://cache-01:5000"
push-to = "ssh://root@cache-01"

[deploy]
strategy = "staged"
health-timeout = 300
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let cp = config.control_plane.unwrap();
        assert_eq!(cp.url, Some("https://cp-01:8080".to_string()));
        assert_eq!(cp.ca_cert, Some("modules/_config/fleet-ca.pem".to_string()));
        let tls = config.tls.unwrap();
        assert_eq!(
            tls.client_cert,
            Some("/run/agenix/agent-${HOSTNAME}-cert".to_string())
        );
        let cache = config.cache.unwrap();
        assert_eq!(cache.url, Some("http://cache-01:5000".to_string()));
        assert_eq!(cache.push_to, Some("ssh://root@cache-01".to_string()));
        let deploy = config.deploy.unwrap();
        assert_eq!(deploy.strategy, Some("staged".to_string()));
        assert_eq!(deploy.health_timeout, Some(300));
    }

    #[test]
    fn test_parse_credentials_file() {
        let toml_str = r#"
["https://cp-01:8080"]
api-key = "nfk-abc123"

["https://prod:8080"]
api-key = "nfk-def456"
"#;
        let creds: CredentialsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            creds.entries.get("https://cp-01:8080").unwrap().api_key,
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
url = "https://cp-01:8080"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let creds = CredentialsFile::default();
        let resolved = resolve(
            Some(&config),
            Some(Path::new("/fleet")),
            &creds,
            CliOverrides {
                cp_url: "https://prod:8080", // CLI override
                ..CliOverrides::default()
            },
        );
        assert_eq!(
            resolved.control_plane_url,
            Some("https://prod:8080".to_string())
        );
    }

    #[test]
    fn test_resolve_default_cli_values_dont_override() {
        let toml_str = r#"
[control-plane]
url = "https://cp-01:8080"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let creds = CredentialsFile::default();
        let resolved = resolve(
            Some(&config),
            Some(Path::new("/fleet")),
            &creds,
            CliOverrides {
                cp_url: "http://localhost:8080", // clap default - should not override
                ..CliOverrides::default()
            },
        );
        assert_eq!(
            resolved.control_plane_url,
            Some("https://cp-01:8080".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_save_api_key_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        // Set both XDG_CONFIG_HOME (Linux) and HOME (macOS) so that
        // dirs::config_dir() resolves to the temp dir on both platforms.
        // On macOS, dirs ignores XDG_CONFIG_HOME and uses ~/Library/Application Support.
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        std::env::set_var("HOME", dir.path());

        save_api_key("https://test.example.com", "nfk-testkey").unwrap();

        let path = credentials_path();
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials file must be 0o600, got {mode:o}");
    }
}

//! Operator-side config: layered resolution + TOML round-trip.
//!
//! Precedence (high → low): explicit flags > NIXFLEET_* env > config file.
//! When every layer leaves a field empty the resolver returns
//! `ConfigError::Missing { field }` so the bin can render a useful hint.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileConfig {
    pub cp_url: Option<String>,
    pub ca_cert: Option<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
}

impl FileConfig {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write 0600 — config holds paths to private key material.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            std::io::Write::write_all(&mut f, body.as_bytes())?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, body)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub cp_url: Option<String>,
    pub ca_cert: Option<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ConfigError {
    Read(std::io::Error),
    Parse(toml::de::Error),
    Missing { field: &'static str },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "read config: {e}"),
            Self::Parse(e) => write!(f, "parse config: {e}"),
            Self::Missing { field } => write!(
                f,
                "no {field} in flags, env, or config file. \
                 Run `nixfleet config init` to create one, or pass --{flag} / set NIXFLEET_{env}.",
                field = field,
                flag = field.replace('_', "-"),
                env = field.to_uppercase(),
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

pub fn load_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let body = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
    toml::from_str(&body).map_err(ConfigError::Parse)
}

pub fn resolve(
    file_path: Option<&Path>,
    env: &Overrides,
    flags: &Overrides,
) -> Result<crate::ResolvedClientConfig, ConfigError> {
    let file = match file_path {
        Some(p) => match load_file(p) {
            Ok(c) => c,
            Err(ConfigError::Read(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                FileConfig::default()
            }
            Err(other) => return Err(other),
        },
        None => FileConfig::default(),
    };
    let cp_url = flags
        .cp_url
        .clone()
        .or_else(|| env.cp_url.clone())
        .or(file.cp_url)
        .ok_or(ConfigError::Missing { field: "cp_url" })?;
    let ca_cert = flags
        .ca_cert
        .clone()
        .or_else(|| env.ca_cert.clone())
        .or(file.ca_cert)
        .ok_or(ConfigError::Missing { field: "ca_cert" })?;
    let client_cert = flags
        .client_cert
        .clone()
        .or_else(|| env.client_cert.clone())
        .or(file.client_cert)
        .ok_or(ConfigError::Missing { field: "client_cert" })?;
    let client_key = flags
        .client_key
        .clone()
        .or_else(|| env.client_key.clone())
        .or(file.client_key)
        .ok_or(ConfigError::Missing { field: "client_key" })?;
    Ok(crate::ResolvedClientConfig {
        cp_url,
        ca_cert,
        client_cert,
        client_key,
    })
}

pub fn default_config_path() -> PathBuf {
    if let Some(base) = dirs::config_dir() {
        return base.join("nixfleet").join("config.toml");
    }
    PathBuf::from(".nixfleet/config.toml")
}

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HealthConfig {
    #[serde(default)]
    pub systemd: Vec<SystemdConfig>,
    #[serde(default)]
    pub http: Vec<HttpConfig>,
    #[serde(default)]
    pub command: Vec<CommandConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemdConfig {
    pub units: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    pub url: String,
    #[serde(default = "default_timeout", alias = "timeout")]
    pub timeout: i64,
    #[serde(default = "default_expected_status", alias = "expectedStatus")]
    pub expected_status: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_cmd_timeout")]
    pub timeout: i64,
}

fn default_timeout() -> i64 {
    3
}
fn default_expected_status() -> i64 {
    200
}
fn default_cmd_timeout() -> i64 {
    5
}

pub fn load_config(path: &str) -> Result<HealthConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read health config: {path}"))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse health config: {path}"))
}

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
    #[serde(default = "default_interval", alias = "interval")]
    #[allow(dead_code)]
    pub interval: i64,
    #[serde(default = "default_timeout", alias = "timeout")]
    pub timeout: i64,
    #[serde(default = "default_expected_status", alias = "expectedStatus")]
    pub expected_status: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_cmd_interval")]
    #[allow(dead_code)]
    pub interval: i64,
    #[serde(default = "default_cmd_timeout")]
    pub timeout: i64,
}

fn default_interval() -> i64 {
    5
}
fn default_timeout() -> i64 {
    3
}
fn default_expected_status() -> i64 {
    200
}
fn default_cmd_interval() -> i64 {
    10
}
fn default_cmd_timeout() -> i64 {
    5
}

pub fn load_config(path: &str) -> Result<HealthConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read health config: {path}"))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse health config: {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let json = r#"{
            "systemd": [{"units": ["nginx.service", "postgresql.service"]}],
            "http": [{"url": "http://localhost:8080/health", "timeout": 5, "expected_status": 200}],
            "command": [{"name": "disk_check", "command": "df -h / | tail -1", "timeout": 10}]
        }"#;
        let cfg: HealthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.systemd.len(), 1);
        assert_eq!(cfg.systemd[0].units.len(), 2);
        assert_eq!(cfg.http.len(), 1);
        assert_eq!(cfg.http[0].url, "http://localhost:8080/health");
        assert_eq!(cfg.http[0].expected_status, 200);
        assert_eq!(cfg.command.len(), 1);
        assert_eq!(cfg.command[0].name, "disk_check");
        assert_eq!(cfg.command[0].timeout, 10);
    }

    #[test]
    fn test_parse_empty_config() {
        let json = "{}";
        let cfg: HealthConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.systemd.is_empty());
        assert!(cfg.http.is_empty());
        assert!(cfg.command.is_empty());
    }

    #[test]
    fn test_defaults_applied() {
        let json = r#"{
            "http": [{"url": "http://localhost/health"}],
            "command": [{"name": "test", "command": "true"}]
        }"#;
        let cfg: HealthConfig = serde_json::from_str(json).unwrap();
        let http = &cfg.http[0];
        assert_eq!(http.interval, 5);
        assert_eq!(http.timeout, 3);
        assert_eq!(http.expected_status, 200);
        let cmd = &cfg.command[0];
        assert_eq!(cmd.interval, 10);
        assert_eq!(cmd.timeout, 5);
    }

    #[test]
    fn test_camel_case_from_nix() {
        let json = r#"{
            "http": [{"url": "http://localhost/health", "expectedStatus": 201, "timeout": 10, "interval": 15}]
        }"#;
        let cfg: HealthConfig = serde_json::from_str(json).unwrap();
        let http = &cfg.http[0];
        assert_eq!(http.expected_status, 201);
        assert_eq!(http.timeout, 10);
        assert_eq!(http.interval, 15);
    }
}

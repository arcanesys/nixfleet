//! Persistent operation log for deploy and release commands.
//!
//! Writes one JSONL file per operation to `~/.local/state/nixfleet/logs/`.
//! Each line is a self-contained JSON object. Events: `op_start`,
//! `subprocess`, `op_end`.

use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Serialize)]
#[serde(tag = "event")]
enum LogEvent<'a> {
    #[serde(rename = "op_start")]
    OpStart {
        ts: String,
        operation: &'a str,
        flake: &'a str,
        hosts: &'a [String],
    },
    #[serde(rename = "subprocess")]
    Subprocess {
        ts: String,
        cmd: &'a [String],
        exit_code: i32,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        stdout: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<&'a str>,
    },
    #[serde(rename = "op_end")]
    OpEnd {
        ts: String,
        success: bool,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
    },
}

pub struct OpLog {
    file: File,
    path: PathBuf,
    start: Instant,
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn log_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state")
        })
        .join("nixfleet/logs")
}

impl OpLog {
    pub fn new(operation: &str) -> Result<Self> {
        let dir = log_dir();
        fs::create_dir_all(&dir).context("failed to create log directory")?;
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let filename = format!("{}_{}.jsonl", ts, operation);
        let path = dir.join(&filename);
        let file = File::create(&path)
            .with_context(|| format!("failed to create log file {}", path.display()))?;
        Ok(Self { file, path, start: Instant::now() })
    }

    pub fn log_start(&mut self, operation: &str, flake: &str, hosts: &[String]) {
        let event = LogEvent::OpStart { ts: now_iso(), operation, flake, hosts };
        self.write_event(&event);
    }

    pub fn log_subprocess(
        &mut self,
        cmd: &[String],
        exit_code: i32,
        duration_ms: u64,
        stdout: Option<&str>,
        stderr: Option<&str>,
        host: Option<&str>,
    ) {
        let event = LogEvent::Subprocess {
            ts: now_iso(), cmd, exit_code, duration_ms, stdout, stderr, host,
        };
        self.write_event(&event);
    }

    pub fn finish(&mut self, success: bool, error: Option<&str>) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let event = LogEvent::OpEnd { ts: now_iso(), success, duration_ms, error };
        self.write_event(&event);
        eprintln!("Log: {}", self.path.display());
    }

    fn write_event(&mut self, event: &LogEvent<'_>) {
        if let Ok(json) = serde_json::to_string(event) {
            let _ = writeln!(self.file, "{}", json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oplog_writes_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        // Override the log dir by creating the OpLog manually
        let path = dir.path().join("test_op.jsonl");
        let file = File::create(&path).unwrap();
        let mut log = OpLog { file, path: path.clone(), start: Instant::now() };

        log.log_start("release_create", ".", &["web-01".to_string()]);
        log.log_subprocess(
            &["nix".to_string(), "eval".to_string()],
            0, 100, Some("/nix/store/abc"), None, Some("web-01"),
        );
        log.finish(true, None);

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "expected 3 JSONL lines, got {}", lines.len());
        for (i, line) in lines.iter().enumerate() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {} is not valid JSON: {}: {}", i, e, line));
            assert!(parsed.get("ts").is_some(), "line {} missing ts", i);
            assert!(parsed.get("event").is_some(), "line {} missing event", i);
        }
        assert!(lines[0].contains("\"event\":\"op_start\""));
        assert!(lines[1].contains("\"event\":\"subprocess\""));
        assert!(lines[2].contains("\"event\":\"op_end\""));
    }
}

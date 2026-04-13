//! Persistent operation log for deploy and release commands.
//!
//! Writes one JSONL file per operation to `~/.local/state/nixfleet/logs/`.
//! Each line is a self-contained JSON object. Events: `op_start`,
//! `subprocess`, `op_end`.
//!
//! Integration pattern: subprocess functions accept `&mut OpLog` and call
//! `oplog.log_output()` after each `display::run_cmd_async()`.
//! The OpLog captures the full `Output` (stdout, stderr, exit code, timing).

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
        cmd: &'a str,
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
        Ok(Self {
            file,
            path,
            start: Instant::now(),
        })
    }

    /// Log the start of an operation.
    pub fn log_start(&mut self, operation: &str, flake: &str, hosts: &[String]) {
        let event = LogEvent::OpStart {
            ts: now_iso(),
            operation,
            flake,
            hosts,
        };
        self.write_event(&event);
    }

    /// Log a subprocess from its raw `Output`, capturing stdout, stderr, exit code, and timing.
    ///
    /// This is the primary logging method. Subprocess functions call this after
    /// `display::run_cmd_async()` with the full `Output`.
    pub fn log_output(
        &mut self,
        cmd_desc: &str,
        host: Option<&str>,
        output: &std::process::Output,
        duration: std::time::Duration,
    ) {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let stdout = if stdout_str.trim().is_empty() {
            None
        } else {
            Some(stdout_str.trim().to_string())
        };
        let stderr = if stderr_str.trim().is_empty() {
            None
        } else {
            Some(stderr_str.trim().to_string())
        };
        let event = LogEvent::Subprocess {
            ts: now_iso(),
            cmd: cmd_desc,
            exit_code: output.status.code().unwrap_or(-1),
            duration_ms: duration.as_millis() as u64,
            stdout: stdout.as_deref(),
            stderr: stderr.as_deref(),
            host,
        };
        self.write_event(&event);
    }

    /// Log the end of the operation. Prints the log path to stderr.
    pub fn finish(&mut self, success: bool, error: Option<&str>) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let event = LogEvent::OpEnd {
            ts: now_iso(),
            success,
            duration_ms,
            error,
        };
        self.write_event(&event);
        if success {
            eprintln!("Log: {}", self.path.display());
        } else {
            eprintln!("Full log: {}", self.path.display());
        }
    }

    fn write_event(&mut self, event: &LogEvent<'_>) {
        match serde_json::to_string(event) {
            Ok(json) => {
                if let Err(e) = writeln!(self.file, "{}", json) {
                    tracing::debug!(error = %e, "oplog write failed");
                }
            }
            Err(e) => tracing::debug!(error = %e, "oplog serialization failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oplog_writes_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_op.jsonl");
        let file = File::create(&path).unwrap();
        let mut log = OpLog {
            file,
            path: path.clone(),
            start: Instant::now(),
        };

        log.log_start("release_create", ".", &["web-01".to_string()]);

        // Simulate a subprocess with a real Output
        let output = std::process::Output {
            status: std::process::Command::new("true").status().unwrap().into(),
            stdout: b"/nix/store/abc-system\n".to_vec(),
            stderr: b"warning: Git tree is dirty\n".to_vec(),
        };
        log.log_output(
            "nix build web-01",
            Some("web-01"),
            &output,
            std::time::Duration::from_millis(100),
        );

        log.finish(true, None);

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "expected 3 JSONL lines, got {}",
            lines.len()
        );
        for (i, line) in lines.iter().enumerate() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {} is not valid JSON: {}: {}", i, e, line));
            assert!(parsed.get("ts").is_some(), "line {} missing ts", i);
            assert!(parsed.get("event").is_some(), "line {} missing event", i);
        }
        // Verify event ordering
        assert!(lines[0].contains("\"event\":\"op_start\""));
        assert!(lines[1].contains("\"event\":\"subprocess\""));
        assert!(lines[2].contains("\"event\":\"op_end\""));

        // Verify subprocess captured stdout and stderr
        let sub: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(sub["stdout"], "/nix/store/abc-system");
        assert_eq!(sub["stderr"], "warning: Git tree is dirty");
        assert_eq!(sub["exit_code"], 0);
        assert_eq!(sub["duration_ms"], 100);
        assert_eq!(sub["host"], "web-01");
    }
}

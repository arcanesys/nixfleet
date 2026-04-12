//! Shared display helpers for CLI output.
//!
//! All formatted terminal output (tables, progress bars, colors,
//! store path truncation) lives here. Subcommand files build data
//! and call these helpers — no hardcoded column widths or separator
//! arithmetic anywhere else.

use comfy_table::{ContentArrangement, Table};
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read as IoRead, Write as IoWrite};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Global verbosity level set by main.rs from the `-v` flag count.
/// 0 = warn (default), 1 = info (-v), 2+ = debug (-vv).
static VERBOSITY: AtomicU8 = AtomicU8::new(0);

/// Set the global verbosity level. Called once from main.rs.
pub fn set_verbosity(level: u8) {
    VERBOSITY.store(level, Ordering::Relaxed);
}

/// Current verbosity level.
pub fn verbosity() -> u8 {
    VERBOSITY.load(Ordering::Relaxed)
}

/// Returns true when verbosity is >= 2 (-vv). Subprocess commands
/// should inherit stdout/stderr instead of piping.
pub fn passthrough_output() -> bool {
    verbosity() >= 2
}



// ---------------------------------------------------------------
// Shared tracing writer
// ---------------------------------------------------------------

/// Global slot for the active MultiProgress. When set, tracing output
/// routes through `MultiProgress::println()` so it appears above the
/// managed progress region.
static SHARED_MULTI: LazyLock<Arc<Mutex<Option<MultiProgress>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

/// A writer that routes through MultiProgress::println() when a
/// RollingWindow is active, or falls back to stderr.
#[derive(Clone)]
pub struct SharedWriter {
    multi: Arc<Mutex<Option<MultiProgress>>>,
}

impl Default for SharedWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedWriter {
    pub fn new() -> Self {
        Self {
            multi: SHARED_MULTI.clone(),
        }
    }
}

impl IoWrite for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let guard = self.multi.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref mp) = *guard {
            let s = String::from_utf8_lossy(buf);
            let trimmed = s.trim_end_matches('\n');
            if !trimmed.is_empty() {
                mp.println(trimmed).ok();
            }
        } else {
            std::io::stderr().write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------
// Rolling window
// ---------------------------------------------------------------

const WINDOW_SIZE: usize = 10;

/// A 10-line rolling window of subprocess output with a progress bar
/// at the bottom, managed via indicatif's MultiProgress.
///
/// INFO lines from tracing route through MultiProgress::println() and
/// appear above the managed region (sticky). Subprocess stderr is fed
/// line-by-line into `log_line()` and displayed in the rolling window.
///
/// On drop: clears everything if no error. If `mark_error()` was called,
/// leaves the rolling lines visible (last 10 lines of output).
pub struct RollingWindow {
    #[allow(dead_code)] // held for Drop side-effects
    multi: MultiProgress,
    lines: Vec<ProgressBar>,
    bar: ProgressBar,
    ring: VecDeque<String>,
    had_error: bool,
}

impl RollingWindow {
    /// Create a new rolling window with a progress bar.
    pub fn new(phase_name: &str, total: u64) -> Self {
        let multi = MultiProgress::new();

        let line_style = ProgressStyle::with_template("  {msg}").unwrap();
        let lines: Vec<ProgressBar> = (0..WINDOW_SIZE)
            .map(|_| {
                let pb = multi.add(ProgressBar::hidden());
                pb.set_style(line_style.clone());
                pb
            })
            .collect();

        let bar_style =
            ProgressStyle::with_template("{spinner} {prefix} {bar:30} {pos}/{len}")
                .unwrap()
                .progress_chars("█▓░");
        let bar = multi.add(ProgressBar::new(total));
        bar.set_style(bar_style);
        bar.set_prefix(phase_name.to_string());
        bar.enable_steady_tick(std::time::Duration::from_millis(120));

        // Install into the shared writer slot
        {
            let mut guard = SHARED_MULTI.lock().unwrap_or_else(|p| p.into_inner());
            *guard = Some(multi.clone());
        }

        Self {
            multi,
            lines,
            bar,
            ring: VecDeque::with_capacity(WINDOW_SIZE),
            had_error: false,
        }
    }

    /// Push a line of subprocess output into the rolling window.
    pub fn log_line(&mut self, text: &str) {
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            return;
        }

        if self.ring.len() >= WINDOW_SIZE {
            self.ring.pop_front();
        }
        self.ring.push_back(trimmed.to_string());

        for (i, pb) in self.lines.iter().enumerate() {
            if let Some(line) = self.ring.get(i) {
                pb.set_message(line.clone());
                pb.set_length(1);
            } else {
                pb.set_message(String::new());
                pb.set_length(0);
            }
        }
    }

    /// Advance the progress bar by one.
    pub fn inc(&self) {
        self.bar.inc(1);
    }

    /// Mark that an error occurred. On drop, rolling lines will be preserved.
    pub fn mark_error(&mut self) {
        self.had_error = true;
    }
}

impl Drop for RollingWindow {
    fn drop(&mut self) {
        // Remove from shared writer first
        {
            let mut guard = SHARED_MULTI.lock().unwrap_or_else(|p| p.into_inner());
            *guard = None;
        }

        if self.had_error {
            self.bar.finish_and_clear();
            for pb in &self.lines {
                pb.finish_and_clear();
            }
            // Print buffered lines as permanent output
            for line in &self.ring {
                eprintln!("  {}", line);
            }
        } else {
            self.bar.finish_and_clear();
            for pb in &self.lines {
                pb.finish_and_clear();
            }
        }
    }
}

// ---------------------------------------------------------------
// Subprocess runner
// ---------------------------------------------------------------

/// Run a command, piping stderr into the rolling window line-by-line.
///
/// - `window = Some(w)`: pipes stderr, feeds each line into `w.log_line()`.
/// - `window = None`: pipes stderr and discards it (quiet mode).
/// - For `-vv` passthrough mode, callers should use `Stdio::inherit()`
///   directly instead of this helper.
pub fn run_cmd(cmd: &mut Command, mut window: Option<&mut RollingWindow>) -> std::io::Result<Output> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

    let stderr_handle = child.stderr.take();
    let mut stderr_buf = Vec::new();
    if let Some(stderr) = stderr_handle {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if let Some(ref mut w) = window {
                        w.log_line(&l);
                    }
                    stderr_buf.extend_from_slice(l.as_bytes());
                    stderr_buf.push(b'\n');
                }
                Err(_) => break,
            }
        }
    }

    let mut stdout_buf = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut stdout_buf)?;
    }

    let status = child.wait()?;

    Ok(Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

/// Async version of `run_cmd` for deploy.rs (uses tokio::process::Command).
pub async fn run_cmd_async(
    cmd: &mut tokio::process::Command,
    mut window: Option<&mut RollingWindow>,
) -> std::io::Result<Output> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

    let stderr_handle = child.stderr.take();
    let mut stderr_buf = Vec::new();
    if let Some(stderr) = stderr_handle {
        use tokio::io::{AsyncBufReadExt, BufReader as TokioBufReader};
        let reader = TokioBufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(ref mut w) = window {
                w.log_line(&line);
            }
            stderr_buf.extend_from_slice(line.as_bytes());
            stderr_buf.push(b'\n');
        }
    }

    let mut stdout_buf = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        use tokio::io::AsyncReadExt;
        stdout.read_to_end(&mut stdout_buf).await?;
    }

    let status = child.wait().await?;

    Ok(std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

// ---------------------------------------------------------------
// Tables
// ---------------------------------------------------------------

/// Render a table with auto-sized columns to stdout.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers);

    for row in rows {
        table.add_row(row);
    }

    println!("{table}");
}

/// Print structured data as JSON or as a table.
pub fn print_list<T: Serialize>(json: bool, headers: &[&str], rows: &[Vec<String>], data: &T) {
    if json {
        match serde_json::to_string_pretty(data) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize JSON: {e}"),
        }
    } else {
        print_table(headers, rows);
    }
}

// ---------------------------------------------------------------
// Store path truncation
// ---------------------------------------------------------------

/// Truncate a Nix store path for display.
pub fn truncate_store_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len || path.is_empty() {
        return path.to_string();
    }

    if let Some(rest) = path.strip_prefix("/nix/store/") {
        if let Some(dash_pos) = rest.find('-') {
            let hash = &rest[..dash_pos.min(7)];
            let name = &rest[dash_pos + 1..];
            let short = format!("/nix/store/{hash}…-{name}");
            if short.len() <= max_len {
                return short;
            }
            let budget = max_len.saturating_sub("/nix/store/…-…".len() + hash.len());
            if budget > 3 {
                return format!(
                    "/nix/store/{hash}…-{}…",
                    &name[..budget.min(name.len())]
                );
            }
        }
    }

    let ellipsis = '…';
    let ellipsis_len = ellipsis.len_utf8();
    let end = max_len.saturating_sub(ellipsis_len);
    format!("{}…", &path[..end.min(path.len())])
}

// ---------------------------------------------------------------
// Status coloring
// ---------------------------------------------------------------

/// Color a status string for terminal display.
pub fn color_status(s: &str) -> String {
    let lower = s.to_lowercase();
    let styled = match lower.as_str() {
        "ok" | "completed" | "healthy" | "succeeded" | "active" => style(s).green(),
        "error" | "failed" | "unhealthy" => style(s).red(),
        "paused" | "pending" | "waiting_health" | "deploying" | "maintenance"
        | "provisioning" => style(s).yellow(),
        _ => style(s).force_styling(false),
    };
    styled.to_string()
}

// ---------------------------------------------------------------
// Key-value detail view
// ---------------------------------------------------------------

/// Print a key-value detail view with aligned labels.
pub fn print_detail(pairs: &[(&str, String)]) {
    let max_key = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in pairs {
        println!(
            "{:<width$}  {}",
            format!("{}:", key),
            value,
            width = max_key + 1
        );
    }
}

// ---------------------------------------------------------------
// Tests
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_path_unchanged() {
        assert_eq!(
            truncate_store_path("/nix/store/abc", 50),
            "/nix/store/abc"
        );
    }

    #[test]
    fn truncate_preserves_hash_and_name() {
        let long =
            "/nix/store/abc123def456ghi789jkl012mno345pqr678-nixos-system-web-01-25.05";
        let result = truncate_store_path(long, 50);
        assert!(
            result.contains("abc123d"),
            "should keep hash prefix: {result}"
        );
        assert!(
            result.contains("nixos-system"),
            "should keep name: {result}"
        );
        assert!(
            result.len() <= 50,
            "should be <=50 chars: {result} ({})",
            result.len()
        );
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate_store_path("", 50), "");
    }

    #[test]
    fn truncate_non_store_path() {
        let long = "a".repeat(60);
        let result = truncate_store_path(&long, 30);
        assert!(result.len() <= 30);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn color_status_returns_string() {
        assert!(!color_status("ok").is_empty());
        assert!(!color_status("failed").is_empty());
        assert!(!color_status("paused").is_empty());
        assert!(!color_status("unknown_value").is_empty());
    }

    #[test]
    fn rolling_window_ring_buffer() {
        let mut ring: VecDeque<String> = VecDeque::with_capacity(WINDOW_SIZE);
        for i in 0..15 {
            if ring.len() >= WINDOW_SIZE {
                ring.pop_front();
            }
            ring.push_back(format!("line {}", i));
        }
        assert_eq!(ring.len(), WINDOW_SIZE);
        assert_eq!(ring.front().unwrap(), "line 5");
        assert_eq!(ring.back().unwrap(), "line 14");
    }

    #[test]
    fn run_cmd_captures_stderr() {
        let output = run_cmd(
            Command::new("sh").args(["-c", "echo hello >&2; echo stdout"]),
            None,
        )
        .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("stdout"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("hello"));
    }
}

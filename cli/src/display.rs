//! Shared display helpers for CLI output.
//!
//! All formatted terminal output (tables, progress bars, colors,
//! store path truncation) lives here. Subcommand files build data
//! and call these helpers — no hardcoded column widths or separator
//! arithmetic anywhere else.

use comfy_table::{ContentArrangement, Table};
use console::style;
use serde::Serialize;

// ---------------------------------------------------------------
// Tables
// ---------------------------------------------------------------

/// Render a table with auto-sized columns to stdout.
///
/// Uses comfy-table: no outer borders, header underline, columns
/// sized to content and constrained to terminal width.
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

/// Print structured data as JSON or as a table, depending on the
/// `json` flag. When `json` is true, `data` is serialized with
/// `serde_json::to_string_pretty`. Otherwise `print_table` is called.
pub fn print_list<T: Serialize>(
    json: bool,
    headers: &[&str],
    rows: &[Vec<String>],
    data: &T,
) {
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
///
/// Preserves the hash prefix and derivation name:
/// `/nix/store/abc1234…-nixos-system-web-01` instead of cutting
/// at an arbitrary byte offset. Falls back to simple prefix
/// truncation for non-store-path strings.
pub fn truncate_store_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len || path.is_empty() {
        return path.to_string();
    }

    // /nix/store/<32-char-hash>-<name>
    if let Some(rest) = path.strip_prefix("/nix/store/") {
        if let Some(dash_pos) = rest.find('-') {
            let hash = &rest[..dash_pos.min(7)];
            let name = &rest[dash_pos + 1..];
            let short = format!("/nix/store/{hash}…-{name}");
            if short.len() <= max_len {
                return short;
            }
            // Name still too long — truncate the name part
            let budget = max_len.saturating_sub("/nix/store/…-…".len() + hash.len());
            if budget > 3 {
                return format!(
                    "/nix/store/{hash}…-{}…",
                    &name[..budget.min(name.len())]
                );
            }
        }
    }

    // Fallback: simple prefix truncation.
    // '…' is 3 bytes in UTF-8, so reserve that many bytes from the budget.
    let ellipsis = '…';
    let ellipsis_len = ellipsis.len_utf8();
    let end = max_len.saturating_sub(ellipsis_len);
    format!("{}…", &path[..end.min(path.len())])
}

// ---------------------------------------------------------------
// Status coloring
// ---------------------------------------------------------------

/// Color a status string for terminal display.
///
/// - Green: ok, completed, healthy, succeeded, active
/// - Red: ERROR, failed, unhealthy
/// - Yellow: paused, pending, waiting_health, deploying, maintenance
///
/// Returns the original string unmodified when stdout is not a TTY.
pub fn color_status(s: &str) -> String {
    let lower = s.to_lowercase();
    let styled = match lower.as_str() {
        "ok" | "completed" | "healthy" | "succeeded" | "active" => {
            style(s).green()
        }
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
///
/// ```text
/// Rollout:         r-abc123
/// Status:          completed
/// Strategy:        canary
/// ```
pub fn print_detail(pairs: &[(&str, String)]) {
    let max_key = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in pairs {
        println!("{:<width$}  {}", format!("{}:", key), value, width = max_key + 1);
    }
}

// Progress bars are managed by the `tracing-indicatif` layer (see main.rs).
// Use `tracing::info_span!("label")` + `span.pb_set_length(n)` /
// `span.pb_inc(1)` from `tracing_indicatif::span_ext::IndicatifSpanExt`.
// Log lines via `tracing::info!` automatically appear above active bars.

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
        assert!(result.contains("abc123d"), "should keep hash prefix: {result}");
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
        // Just verify it doesn't panic and returns non-empty
        assert!(!color_status("ok").is_empty());
        assert!(!color_status("failed").is_empty());
        assert!(!color_status("paused").is_empty());
        assert!(!color_status("unknown_value").is_empty());
    }
}

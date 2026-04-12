//! Shared glob matching for CLI host filtering.
//!
//! A deliberately minimal implementation: `*` matches any sequence of
//! characters, no other wildcards are recognized. Used by both the
//! `deploy` and `release` subcommands to intersect hostnames with the
//! `--hosts <pattern>` flag.
//!
//! Byte-offset indexing (`text[pos..]`) is safe here because `pos` is
//! only ever advanced by `idx + part.len()`, both of which are UTF-8
//! character boundaries returned by `str::find`.

/// Match `text` against a glob `pattern`.
///
/// - `"*"` matches anything.
/// - `"web-*"` matches any string beginning with `"web-"`.
/// - `"*-01"` matches any string ending with `"-01"`.
/// - `"web-*-prod"` matches strings beginning with `"web-"`, containing
///   any intermediate characters, and ending with `"-prod"`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        // No wildcard — exact match.
        return pattern == text;
    }

    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) => {
                // First segment must anchor at the start of `text`.
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }

    // If the pattern does not end with `*`, the last segment must
    // anchor at the end of the text.
    if !parts.last().copied().unwrap_or("").is_empty() {
        pos == text.len()
    } else {
        true
    }
}

/// Filter a list of hostnames by glob pattern, returning the matches
/// in the original order. `"*"` returns everything.
pub fn filter_hosts(hosts: &[String], pattern: &str) -> Vec<String> {
    if pattern == "*" {
        return hosts.to_vec();
    }
    hosts
        .iter()
        .filter(|h| glob_match(pattern, h))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_matches_everything() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn exact_no_wildcard() {
        assert!(glob_match("web-01", "web-01"));
        assert!(!glob_match("web-01", "web-02"));
    }

    #[test]
    fn prefix_match() {
        assert!(glob_match("web-*", "web-01"));
        assert!(glob_match("web-*", "web-"));
        assert!(!glob_match("web-*", "db-01"));
    }

    #[test]
    fn suffix_match() {
        assert!(glob_match("*-prod", "web-prod"));
        assert!(glob_match("*-prod", "-prod"));
        assert!(!glob_match("*-prod", "web-dev"));
    }

    #[test]
    fn mid_wildcard() {
        assert!(glob_match("web-*-prod", "web-01-prod"));
        assert!(!glob_match("web-*-prod", "web-01-dev"));
    }

    #[test]
    fn filter_hosts_preserves_order() {
        let hosts = vec!["db-01".into(), "web-01".into(), "web-02".into()];
        let filtered = filter_hosts(&hosts, "web-*");
        assert_eq!(filtered, vec!["web-01".to_string(), "web-02".to_string()]);
    }
}

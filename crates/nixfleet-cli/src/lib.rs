//! Shared CLI logic — table rendering, age math, status classification.
//! Kept as a library so binaries (`nixfleet status` today, `rollout
//! trace` + `diff` next) compose against it and unit tests can exercise
//! formatting without spinning up a real CP.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use nixfleet_proto::{HostStatusEntry, RolloutTrace};

pub struct StatusInputs {
    pub now: DateTime<Utc>,
    pub hosts: Vec<HostStatusEntry>,
    /// channel name → freshness_window in minutes (from `/v1/channels/{name}`).
    pub channel_freshness: BTreeMap<String, u32>,
}

pub fn render_status_table(input: &StatusInputs) -> String {
    let mut rows: Vec<[String; 6]> = Vec::with_capacity(input.hosts.len() + 1);
    rows.push([
        "HOST".into(),
        "CHANNEL".into(),
        "CURRENT".into(),
        "DECLARED".into(),
        "STATUS".into(),
        "COMPLIANCE".into(),
    ]);
    for host in &input.hosts {
        rows.push([
            host.hostname.clone(),
            host.channel.clone(),
            display_hash(host.current_closure_hash.as_deref(), "<unseen>"),
            display_hash(host.declared_closure_hash.as_deref(), "<unset>"),
            status_label(
                host,
                input.now,
                input.channel_freshness.get(&host.channel).copied(),
            ),
            compliance_label(host),
        ]);
    }

    let mut widths = [0usize; 6];
    for row in &rows {
        for (i, col) in row.iter().enumerate() {
            widths[i] = widths[i].max(col.chars().count());
        }
    }

    let mut out = String::new();
    for row in &rows {
        for (i, col) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(col);
            if i + 1 < row.len() {
                let pad = widths[i].saturating_sub(col.chars().count());
                for _ in 0..pad {
                    out.push(' ');
                }
            }
        }
        out.push('\n');
    }
    out
}

fn display_hash(h: Option<&str>, fallback: &str) -> String {
    match h {
        None => fallback.to_string(),
        Some(s) if s.chars().count() <= 14 => s.to_string(),
        Some(s) => {
            let prefix: String = s.chars().take(13).collect();
            format!("{prefix}\u{2026}")
        }
    }
}

fn status_label(
    host: &HostStatusEntry,
    now: DateTime<Utc>,
    freshness_minutes: Option<u32>,
) -> String {
    let base = base_status_label(host, now, freshness_minutes);
    // Issue #88: a pin is operator-declared metadata, not a status of its
    // own. Appending it as a suffix preserves the existing health signal
    // (converged / failed / stale / etc.) while making the freeze visible
    // at a glance. Short-prefix the commit to keep the column tidy.
    match host.pin.as_ref() {
        Some(pin) => {
            let short: String = pin.commit.chars().take(7).collect();
            format!("{base} \u{1F512}{short}")
        }
        None => base,
    }
}

fn base_status_label(
    host: &HostStatusEntry,
    now: DateTime<Utc>,
    freshness_minutes: Option<u32>,
) -> String {
    use nixfleet_proto::HostRolloutState;

    // Failed/Reverted is louder than closure-match because the rollout's
    // state machine remembers the failure even after operator-driven
    // recovery — surface it.
    if let Some(state) = host.rollout_state {
        if state.is_failed() {
            return match state {
                HostRolloutState::Failed => "\u{2717} failed".to_string(),
                HostRolloutState::Reverted => "\u{2717} reverted".to_string(),
                _ => format!("\u{2717} {}", state.as_db_str().to_lowercase()),
            };
        }
    }

    // Quarantined ranks above pending-reboot: a quarantined host is stuck
    // on a known-broken closure and needs a CI-side fix, not an operator
    // action on the host itself. Pending-reboot is recoverable by reboot;
    // quarantine requires upstream intervention.
    if host.quarantined_closure.is_some() {
        return "\u{2717} quarantined".to_string();
    }

    // Pending-reboot is operator-actionable: agent set the new profile but a
    // critical-component swap forced a reboot. Surface louder than in-progress
    // states so it doesn't get lost in the noise.
    if host.pending_reboot {
        return "\u{27F3} pending reboot".to_string();
    }

    if host.converged {
        return "\u{2713} converged".to_string();
    }

    // No checkin yet — host hasn't reached the CP since the rollout opened.
    let Some(last) = host.last_checkin_at else {
        return "\u{2717} never".to_string();
    };

    // Stale-checkin trumps in-flight state — a host stuck in `Activating`
    // for 3 days isn't "activating", it's offline.
    if let Some(window) = freshness_minutes {
        let age = now.signed_duration_since(last);
        let stale_threshold = chrono::Duration::minutes(i64::from(window) * 2);
        if age > stale_threshold {
            return format!("\u{26A0} stale ({})", format_age(age));
        }
    }

    // Fresh checkin + non-failed state → use the state machine if present.
    match host.rollout_state {
        Some(s) if s.is_terminal_for_ordering() => format!(
            "\u{2713} {}",
            s.as_db_str().to_lowercase(),
        ),
        Some(s) if s.is_in_flight() => format!(
            "\u{2192} {}",
            s.as_db_str().to_lowercase(),
        ),
        Some(HostRolloutState::Queued) => "\u{2026} queued".to_string(),
        _ => "\u{2192} in progress".to_string(),
    }
}

fn format_age(d: chrono::Duration) -> String {
    let total_seconds = d.num_seconds().max(0);
    if total_seconds >= 86400 {
        format!("{}d", total_seconds / 86400)
    } else if total_seconds >= 3600 {
        format!("{}h", total_seconds / 3600)
    } else {
        format!("{}m", total_seconds / 60)
    }
}

fn compliance_label(host: &HostStatusEntry) -> String {
    // Issue #86: include health-probe failures in the same column.
    // Compliance + runtime-gate + health all surface as "outstanding"
    // so the operator gets one number to react to. Drill-down lives
    // in the dashboard / `/v1/hosts` JSON.
    let total = host.outstanding_compliance_failures
        + host.outstanding_runtime_gate_errors
        + host.outstanding_health_failures;
    format!("{total} outstanding")
}

/// Render `nixfleet rollout trace` output: wave-major listing of every
/// dispatch_history row for a rollout. Open dispatches show `<open>`
/// in the TERMINAL column; the operator reads the table top-to-bottom
/// to follow the rollout through waves.
pub fn render_trace_table(trace: &RolloutTrace) -> String {
    let mut rows: Vec<[String; 5]> = Vec::with_capacity(trace.events.len() + 1);
    rows.push([
        "WAVE".into(),
        "HOST".into(),
        "DISPATCHED".into(),
        "TERMINAL".into(),
        "AT".into(),
    ]);
    for ev in &trace.events {
        rows.push([
            ev.wave.to_string(),
            ev.host.clone(),
            short_ts(&ev.dispatched_at),
            ev.terminal_state.clone().unwrap_or_else(|| "<open>".into()),
            ev.terminal_at
                .as_deref()
                .map(short_ts)
                .unwrap_or_default(),
        ]);
    }

    let mut widths = [0usize; 5];
    for row in &rows {
        for (i, col) in row.iter().enumerate() {
            widths[i] = widths[i].max(col.chars().count());
        }
    }

    let mut out = format!("rollout {}\n", trace.rollout_id);
    for row in &rows {
        for (i, col) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(col);
            if i + 1 < row.len() {
                let pad = widths[i].saturating_sub(col.chars().count());
                for _ in 0..pad {
                    out.push(' ');
                }
            }
        }
        out.push('\n');
    }
    out
}

/// "2026-05-05T12:34:56.789Z" → "2026-05-05 12:34:56" (drop subseconds +
/// zone for a denser column). Falls back to the original on parse fail
/// so malformed historical rows surface to the operator.
fn short_ts(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixture_host(
        hostname: &str,
        channel: &str,
        converged: bool,
        last_checkin_min_ago: Option<i64>,
        outstanding: usize,
    ) -> HostStatusEntry {
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        HostStatusEntry {
            hostname: hostname.into(),
            channel: channel.into(),
            declared_closure_hash: Some("aaaaaaaaaaaaaaaaaaaa".into()),
            current_closure_hash: last_checkin_min_ago
                .map(|_| "bbbbbbbbbbbbbbbbbbbb".to_string()),
            pending_closure_hash: None,
            last_checkin_at: last_checkin_min_ago.map(|m| now - chrono::Duration::minutes(m)),
            last_rollout_id: None,
            converged,
            outstanding_compliance_failures: outstanding,
            outstanding_runtime_gate_errors: 0,
            verified_event_count: 0,
            last_uptime_secs: None,
            rollout_state: None,
            pending_reboot: false,
            quarantined_closure: None,
            pin: None,
            outstanding_health_failures: 0,
        }
    }

    #[test]
    fn renders_three_status_classes() {
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let inputs = StatusInputs {
            now,
            hosts: vec![
                fixture_host("lab", "stable", true, Some(0), 0),
                fixture_host("krach", "stable", false, None, 0),
                fixture_host("ohm", "stable", false, Some(60 * 24 * 3), 2),
            ],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2713} converged"), "no converged: {out}");
        assert!(out.contains("\u{2717} never"), "no never: {out}");
        assert!(out.contains("\u{26A0} stale (3d)"), "no stale: {out}");
        assert!(out.contains("HOST"));
        assert!(out.contains("0 outstanding"));
        assert!(out.contains("2 outstanding"));
    }

    #[test]
    fn long_hashes_truncate_with_ellipsis() {
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", true, Some(0), 0);
        h.declared_closure_hash = Some("0123456789abcdef0123456789abcdef".into());
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::new(),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("0123456789abc\u{2026}"), "no truncation: {out}");
    }

    #[test]
    fn missing_freshness_window_skips_staleness_check() {
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let inputs = StatusInputs {
            now,
            hosts: vec![fixture_host("a", "stable", false, Some(60 * 24 * 7), 0)],
            channel_freshness: BTreeMap::new(),
        };
        let out = render_status_table(&inputs);
        assert!(
            out.contains("\u{2192} in progress"),
            "fell through to in-progress without a window: {out}"
        );
        assert!(!out.contains("stale"), "shouldn't be stale without window: {out}");
    }

    #[test]
    fn quarantined_renders_above_pending_reboot_priority() {
        // Quarantine + pending-reboot can't actually co-occur on the same
        // host (different code paths) but the priority ordering is a
        // contract: quarantine wins because it requires CI-side intervention
        // (the closure is broken) rather than just an operator reboot.
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.quarantined_closure = Some("broken-closure-h1".into());
        h.pending_reboot = true;
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(
            out.contains("\u{2717} quarantined"),
            "expected quarantined label: {out}",
        );
        assert!(
            !out.contains("pending reboot"),
            "quarantined must out-rank pending-reboot: {out}",
        );
    }

    #[test]
    fn health_failures_roll_into_outstanding_count() {
        // Issue #86: outstanding_health_failures sums into the COMPLIANCE
        // column alongside compliance + runtime-gate counts. Operator
        // gets one number per host; drill-down lives in the dashboard.
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", true, Some(0), 1); // 1 compliance
        h.outstanding_runtime_gate_errors = 1;
        h.outstanding_health_failures = 2;
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        // 1 compliance + 1 runtime-gate + 2 health = 4
        assert!(out.contains("4 outstanding"), "expected combined count: {out}");
    }

    #[test]
    fn pin_appends_to_converged_label() {
        // Pin is operator metadata, not a health signal — it AUGMENTS
        // the existing label rather than supplanting it. A pinned-and-
        // converged host shows "✓ converged 🔒<short>".
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", true, Some(0), 0);
        h.pin = Some(nixfleet_proto::Pin {
            commit: "abc12345-deadbeef".into(),
            reason: "investigating CVE".into(),
            expires_at: None,
        });
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2713} converged"), "must keep converged: {out}");
        assert!(out.contains("\u{1F512}abc1234"), "must show 7-char pin prefix: {out}");
        assert!(!out.contains("abc12345"), "8th char must be truncated: {out}");
    }

    #[test]
    fn pin_appends_to_failed_label_too() {
        // Even on failure paths the pin info is visible — operator
        // wants to know "this host was supposed to be on commit X
        // and it's failed".
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.rollout_state = Some(HostRolloutState::Failed);
        h.pin = Some(nixfleet_proto::Pin {
            commit: "frozen1".into(),
            reason: "Q2 audit".into(),
            expires_at: None,
        });
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2717} failed"));
        assert!(out.contains("\u{1F512}frozen1"));
    }

    #[test]
    fn pending_reboot_renders_distinctly_when_not_converged() {
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.pending_reboot = true;
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(
            out.contains("\u{27F3} pending reboot"),
            "expected pending-reboot label: {out}",
        );
        assert!(!out.contains("converged"), "should not show converged: {out}");
        assert!(!out.contains("in progress"), "pending-reboot is louder than in-progress: {out}");
    }

    #[test]
    fn rollout_state_failed_takes_priority_over_converged() {
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", true, Some(1), 0);
        h.rollout_state = Some(HostRolloutState::Failed);
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2717} failed"), "expected failed label: {out}");
        assert!(!out.contains("converged"), "should not show converged: {out}");
    }

    #[test]
    fn rollout_state_in_flight_renders_active_state() {
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.rollout_state = Some(HostRolloutState::Activating);
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2192} activating"), "expected activating: {out}");
    }

    #[test]
    fn rollout_state_soaked_renders_as_terminal() {
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.rollout_state = Some(HostRolloutState::Soaked);
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(
            out.contains("\u{2713} soaked"),
            "expected soaked terminal label: {out}"
        );
    }

    #[test]
    fn rollout_state_queued_renders_distinctly() {
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(1), 0);
        h.rollout_state = Some(HostRolloutState::Queued);
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(out.contains("\u{2026} queued"), "expected queued label: {out}");
    }

    fn trace_event(host: &str, wave: u32, terminal: Option<&str>) -> nixfleet_proto::RolloutTraceEvent {
        nixfleet_proto::RolloutTraceEvent {
            host: host.into(),
            channel: "stable".into(),
            wave,
            target_closure_hash: "system-r1".into(),
            target_channel_ref: "stable@trace1".into(),
            dispatched_at: "2026-05-05T12:00:00Z".into(),
            terminal_state: terminal.map(String::from),
            terminal_at: terminal.map(|_| "2026-05-05T12:30:00Z".into()),
        }
    }

    #[test]
    fn render_trace_table_shows_open_dispatches_distinctly() {
        let trace = RolloutTrace {
            rollout_id: "stable@trace1".into(),
            events: vec![
                trace_event("lab", 0, Some("converged")),
                trace_event("krach", 1, None),
            ],
        };
        let out = render_trace_table(&trace);
        assert!(out.contains("rollout stable@trace1"), "missing header: {out}");
        assert!(out.contains("WAVE"), "missing column header: {out}");
        assert!(out.contains("converged"), "missing terminal state: {out}");
        assert!(out.contains("<open>"), "missing open marker: {out}");
        assert!(
            out.contains("2026-05-05 12:00:00"),
            "timestamp not shortened: {out}"
        );
    }

    #[test]
    fn stale_checkin_overrides_in_flight_state() {
        use nixfleet_proto::HostRolloutState;
        let now = Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap();
        let mut h = fixture_host("a", "stable", false, Some(60 * 24 * 3), 0);
        h.rollout_state = Some(HostRolloutState::Activating);
        let inputs = StatusInputs {
            now,
            hosts: vec![h],
            channel_freshness: BTreeMap::from([("stable".to_string(), 180)]),
        };
        let out = render_status_table(&inputs);
        assert!(
            out.contains("\u{26A0} stale"),
            "stale should win over in-flight Activating: {out}"
        );
    }
}

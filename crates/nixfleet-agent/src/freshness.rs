//! Defense-in-depth: refuse targets whose backing fleet.resolved is older
//! than the channel's freshness window (CP applies the same gate at tick start).

use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::EvaluatedTarget;

/// LOADBEARING: clock-skew slack added to the freshness window - without it,
/// a target signed `window` seconds ago is rejected the instant the agent's
/// clock is ahead by 1s.
pub const CLOCK_SKEW_SLACK_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessCheck {
    Fresh,
    /// Caller must refuse activation and post `StaleTarget`.
    Stale {
        signed_at: DateTime<Utc>,
        freshness_window_secs: u32,
        age_secs: i64,
    },
}

pub fn check(target: &EvaluatedTarget, now: DateTime<Utc>) -> FreshnessCheck {
    let age_secs = (now - target.signed_at).num_seconds();
    let limit = target.freshness_window_secs as i64 + CLOCK_SKEW_SLACK_SECS;

    if age_secs > limit {
        FreshnessCheck::Stale {
            signed_at: target.signed_at,
            freshness_window_secs: target.freshness_window_secs,
            age_secs,
        }
    } else {
        FreshnessCheck::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn target_with(signed_at: DateTime<Utc>, window: u32) -> EvaluatedTarget {
        EvaluatedTarget {
            closure_hash: "h".into(),
            channel_ref: "stable@abc".into(),
            evaluated_at: Utc::now(),
            rollout_id: "stable@abc".into(),
            wave_index: None,
            activate: None,
            signed_at,
            freshness_window_secs: window,
            compliance_mode: None,
        }
    }

    #[test]
    fn fresh_when_age_well_under_window() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed + chrono::Duration::seconds(100);
        let t = target_with(signed, 3600);
        assert_eq!(check(&t, now), FreshnessCheck::Fresh);
    }

    #[test]
    fn fresh_at_exact_window_boundary() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed + chrono::Duration::seconds(3600);
        let t = target_with(signed, 3600);
        assert_eq!(check(&t, now), FreshnessCheck::Fresh);
    }

    #[test]
    fn fresh_within_slack_past_window() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed + chrono::Duration::seconds(3660);
        let t = target_with(signed, 3600);
        assert_eq!(check(&t, now), FreshnessCheck::Fresh);
    }

    #[test]
    fn stale_just_past_slack() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed + chrono::Duration::seconds(3661);
        let t = target_with(signed, 3600);
        assert!(matches!(
            check(&t, now),
            FreshnessCheck::Stale { age_secs: 3661, .. }
        ));
    }

    #[test]
    fn stale_far_past_window() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed + chrono::Duration::seconds(86_400 * 3);
        let t = target_with(signed, 3600);
        let result = check(&t, now);
        match result {
            FreshnessCheck::Stale {
                age_secs,
                freshness_window_secs,
                ..
            } => {
                assert_eq!(freshness_window_secs, 3600);
                assert_eq!(age_secs, 86_400 * 3);
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn fresh_when_clock_skew_slightly_negative() {
        let signed = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let now = signed - chrono::Duration::seconds(30);
        let t = target_with(signed, 3600);
        assert_eq!(check(&t, now), FreshnessCheck::Fresh);
    }
}

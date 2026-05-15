//! Cross-module test fixtures.

use chrono::{DateTime, Utc};

use super::Db;
use super::host_dispatch_state::DispatchInsert;
use super::reports::HostReportInsert;
use crate::state::{HealthyMarker, HostRolloutState};

pub(crate) fn fresh_db() -> Db {
    let db = Db::open_in_memory().unwrap();
    db.migrate().unwrap();
    db
}

pub(crate) fn mark_healthy(db: &Db, host: &str, rollout: &str, now: DateTime<Utc>) {
    db.rollout_state()
        .transition_host_state(
            host,
            rollout,
            HostRolloutState::Healthy,
            HealthyMarker::Set(now),
            None,
        )
        .unwrap();
}

pub(crate) fn dispatch_insert<'a>(
    host: &'a str,
    rollout: &'a str,
    target_closure: &'a str,
    deadline: DateTime<Utc>,
) -> DispatchInsert<'a> {
    DispatchInsert {
        hostname: host,
        rollout_id: rollout,
        channel: "stable",
        wave: 0,
        target_closure_hash: target_closure,
        target_channel_ref: rollout,
        confirm_deadline: deadline,
    }
}

pub(crate) fn fail_event<'a>(
    rollout: Option<&'a str>,
    sig: Option<&'a str>,
) -> HostReportInsert<'a> {
    HostReportInsert {
        hostname: "host-05",
        event_id: "evt-test",
        received_at: Utc::now(),
        event_kind: "compliance-failure",
        rollout,
        signature_status: sig,
        report_json: r#"{"hostname":"host-05","agentVersion":"test"}"#,
    }
}

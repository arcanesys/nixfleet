//! Durable per-host event log (soft state); loss is bounded by re-posts.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::sync::Mutex;

/// `signature_status` is the raw kebab-case `SignatureStatus` serde rep.
#[derive(Debug, Clone)]
pub struct HostReportRow {
    pub event_id: String,
    pub received_at: DateTime<Utc>,
    pub event_kind: String,
    pub rollout: Option<String>,
    pub signature_status: Option<String>,
    pub report_json: String,
}

#[derive(Debug, Clone)]
pub struct HostReportInsert<'a> {
    pub hostname: &'a str,
    pub event_id: &'a str,
    pub received_at: DateTime<Utc>,
    pub event_kind: &'a str,
    pub rollout: Option<&'a str>,
    pub signature_status: Option<&'a str>,
    pub report_json: &'a str,
}

pub struct Reports<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

impl Reports<'_> {
    pub fn record_host_report(&self, row: &HostReportInsert<'_>) -> Result<()> {
        super::read(self.conn, |c| {
            c.execute(
                "INSERT INTO host_reports
                   (hostname, event_id, received_at, event_kind,
                    rollout, signature_status, report_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.hostname,
                    row.event_id,
                    row.received_at.to_rfc3339(),
                    row.event_kind,
                    row.rollout,
                    row.signature_status,
                    row.report_json
                ],
            )
            .context("insert host_reports")?;
            Ok(())
        })
    }

    /// Up to `limit_per_host` most-recent rows in chronological order (oldest first).
    pub fn host_reports_recent_per_host(
        &self,
        hostname: &str,
        limit_per_host: usize,
    ) -> Result<Vec<HostReportRow>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT event_id, received_at, event_kind, rollout, signature_status, report_json
                 FROM host_reports
                 WHERE hostname = ?1
                 ORDER BY received_at DESC
                 LIMIT ?2",
            )?;
            let mut rows: Vec<HostReportRow> = stmt
                .query_map(params![hostname, limit_per_host as i64], |row| {
                    let received_str: String = row.get(1)?;
                    let received_at = received_str.parse::<DateTime<Utc>>().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                    Ok(HostReportRow {
                        event_id: row.get::<_, String>(0)?,
                        received_at,
                        event_kind: row.get::<_, String>(2)?,
                        rollout: row.get::<_, Option<String>>(3)?,
                        signature_status: row.get::<_, Option<String>>(4)?,
                        report_json: row.get::<_, String>(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("query host_reports")?;
            // DB DESC -> ring buffer ASC.
            rows.reverse();
            Ok(rows)
        })
    }

    /// Fleet-wide most-recent rows in DESC chronological order; backs
    /// `/v1/host-reports` (durable ring authoritative across journal rotation).
    pub fn recent_across_hosts(&self, limit: usize) -> Result<Vec<(String, HostReportRow)>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT hostname, event_id, received_at, event_kind,
                        rollout, signature_status, report_json
                 FROM host_reports
                 ORDER BY received_at DESC, id DESC
                 LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit as i64], |row| {
                    let hostname: String = row.get(0)?;
                    let received_str: String = row.get(2)?;
                    let received_at = received_str.parse::<DateTime<Utc>>().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                    Ok((
                        hostname,
                        HostReportRow {
                            event_id: row.get::<_, String>(1)?,
                            received_at,
                            event_kind: row.get::<_, String>(3)?,
                            rollout: row.get::<_, Option<String>>(4)?,
                            signature_status: row.get::<_, Option<String>>(5)?,
                            report_json: row.get::<_, String>(6)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("query host_reports recent_across_hosts")?;
            Ok(rows)
        })
    }

    pub fn host_reports_known_hostnames(&self) -> Result<Vec<String>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare("SELECT DISTINCT hostname FROM host_reports")?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("query host_reports hostnames")?;
            Ok(names)
        })
    }

    pub fn prune_host_reports(&self, max_age_hours: i64) -> Result<usize> {
        super::read(self.conn, |c| {
            c.execute(
                "DELETE FROM host_reports
                 WHERE received_at < datetime('now', ?1)",
                params![format!("-{max_age_hours} hours")],
            )
            .context("prune host_reports")
        })
    }

    /// Per-(rollout, host) counts; per-rollout grouping enforces resolution-by-replacement.
    /// `mismatch` and `malformed` excluded: they could be forged FAIL events from a stolen cert.
    pub fn outstanding_compliance_events_by_rollout(
        &self,
    ) -> Result<HashMap<String, HashMap<String, usize>>> {
        super::read(self.conn, |c| {
            let mut stmt = c.prepare(
                "SELECT rollout, hostname, COUNT(*) FROM host_reports
                 WHERE rollout IS NOT NULL
                   AND event_kind IN ('compliance-failure', 'runtime-gate-error')
                   AND COALESCE(signature_status, '') NOT IN ('mismatch', 'malformed')
                 GROUP BY rollout, hostname",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as usize,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("query outstanding_compliance_events_by_rollout")?;
            let mut out: HashMap<String, HashMap<String, usize>> = HashMap::new();
            for (rollout, host, n) in rows {
                out.entry(rollout).or_default().insert(host, n);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::{fail_event, fresh_db};
    use super::HostReportInsert;
    use chrono::Utc;

    #[test]
    fn host_reports_round_trip_preserves_envelope() {
        let db = fresh_db();
        let row = HostReportInsert {
            hostname: "host-05",
            event_id: "evt-rt-1",
            received_at: Utc::now(),
            event_kind: "compliance-failure",
            rollout: Some("edge-slow@abc"),
            signature_status: Some("verified"),
            report_json: r#"{"hostname":"host-05","agentVersion":"0.2.0"}"#,
        };
        db.reports().record_host_report(&row).unwrap();
        let mut got = db
            .reports()
            .host_reports_recent_per_host("host-05", 8)
            .unwrap();
        assert_eq!(got.len(), 1);
        let r = got.pop().unwrap();
        assert_eq!(r.event_id, "evt-rt-1");
        assert_eq!(r.event_kind, "compliance-failure");
        assert_eq!(r.rollout.as_deref(), Some("edge-slow@abc"));
        assert_eq!(r.signature_status.as_deref(), Some("verified"));
    }

    #[test]
    fn outstanding_events_by_rollout_filters_tampered() {
        let db = fresh_db();
        for (eid, sig) in [
            ("e1", Some("verified")),
            ("e2", Some("unsigned")),
            ("e3", Some("no-pubkey")),
            ("e4", Some("mismatch")),
            ("e5", Some("malformed")),
        ] {
            let mut row = fail_event(Some("R1"), sig);
            row.event_id = eid;
            db.reports().record_host_report(&row).unwrap();
        }
        let by_rollout = db
            .reports()
            .outstanding_compliance_events_by_rollout()
            .unwrap();
        assert_eq!(
            by_rollout.get("R1").and_then(|m| m.get("host-05")).copied(),
            Some(3),
        );
    }

    #[test]
    fn outstanding_events_by_rollout_groups_per_rollout() {
        let db = fresh_db();
        let mut e0 = fail_event(Some("R0"), Some("verified"));
        e0.event_id = "evt-r0-1";
        db.reports().record_host_report(&e0).unwrap();
        let mut e1 = fail_event(Some("R1"), Some("verified"));
        e1.event_id = "evt-r1-1";
        db.reports().record_host_report(&e1).unwrap();
        let by_rollout = db
            .reports()
            .outstanding_compliance_events_by_rollout()
            .unwrap();
        assert_eq!(
            by_rollout.get("R0").and_then(|m| m.get("host-05")).copied(),
            Some(1),
        );
        assert_eq!(
            by_rollout.get("R1").and_then(|m| m.get("host-05")).copied(),
            Some(1),
        );
    }

    #[test]
    fn outstanding_events_by_rollout_excludes_null_rollout() {
        let db = fresh_db();
        let mut row = fail_event(None, Some("verified"));
        row.event_id = "evt-orphan";
        db.reports().record_host_report(&row).unwrap();
        let by_rollout = db
            .reports()
            .outstanding_compliance_events_by_rollout()
            .unwrap();
        assert!(
            by_rollout.is_empty(),
            "rollout=NULL events should not appear: {:?}",
            by_rollout,
        );
    }

    #[test]
    fn recent_across_hosts_returns_newest_first_across_all_hosts() {
        let db = fresh_db();
        let now = Utc::now();
        // Three events on two hosts at distinct receive times.
        for (i, (host, eid, kind)) in [
            ("host-05", "evt-1-oldest", "activation-started"),
            ("host-01", "evt-2-mid", "compliance-failure"),
            ("host-02", "evt-3-newest", "rollback-triggered"),
        ]
        .iter()
        .enumerate()
        {
            db.reports()
                .record_host_report(&HostReportInsert {
                    hostname: host,
                    event_id: eid,
                    received_at: now - chrono::Duration::seconds(10 - (i as i64) * 5),
                    event_kind: kind,
                    rollout: None,
                    signature_status: None,
                    report_json: "{}",
                })
                .unwrap();
        }
        let rows = db.reports().recent_across_hosts(10).unwrap();
        assert_eq!(rows.len(), 3);
        // DESC chronological - newest first regardless of host.
        let event_ids: Vec<&str> = rows.iter().map(|(_, r)| r.event_id.as_str()).collect();
        assert_eq!(event_ids, vec!["evt-3-newest", "evt-2-mid", "evt-1-oldest"]);
        // Hostname is the first tuple element so callers can filter without
        // re-parsing the JSON envelope.
        let hosts: Vec<&str> = rows.iter().map(|(h, _)| h.as_str()).collect();
        assert_eq!(hosts, vec!["host-02", "host-01", "host-05"]);
    }

    #[test]
    fn recent_across_hosts_clamps_to_limit() {
        let db = fresh_db();
        let now = Utc::now();
        for i in 0..5 {
            db.reports()
                .record_host_report(&HostReportInsert {
                    hostname: "host-05",
                    event_id: &format!("evt-{i}"),
                    received_at: now - chrono::Duration::seconds(i),
                    event_kind: "activation-started",
                    rollout: None,
                    signature_status: None,
                    report_json: "{}",
                })
                .unwrap();
        }
        let rows = db.reports().recent_across_hosts(2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1.event_id, "evt-0");
        assert_eq!(rows[1].1.event_id, "evt-1");
    }

    #[test]
    fn prune_host_reports_drops_old_rows() {
        let db = fresh_db();
        let past = Utc::now() - chrono::Duration::hours(48);
        let row = HostReportInsert {
            hostname: "host-05",
            event_id: "evt-old",
            received_at: past,
            event_kind: "compliance-failure",
            rollout: None,
            signature_status: None,
            report_json: "{}",
        };
        db.reports().record_host_report(&row).unwrap();
        let n = db.reports().prune_host_reports(24).unwrap();
        assert_eq!(n, 1);
        let names = db.reports().host_reports_known_hostnames().unwrap();
        assert!(names.is_empty(), "old row should be pruned");
    }
}

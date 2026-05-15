//! Agent-cert revocation list (hard state); replayed each tick from signed sidecar.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::sync::Mutex;

pub struct Revocations<'a> {
    pub(super) conn: &'a Mutex<Connection>,
}

impl Revocations<'_> {
    /// Upsert: any cert with notBefore < `not_before` is rejected; re-revoking moves it forward.
    pub fn revoke_cert(
        &self,
        hostname: &str,
        not_before: DateTime<Utc>,
        reason: Option<&str>,
        revoked_by: Option<&str>,
    ) -> Result<()> {
        super::read(self.conn, |c| {
            c.execute(
                "INSERT INTO cert_revocations(hostname, not_before, reason, revoked_by)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(hostname) DO UPDATE SET
                   not_before = excluded.not_before,
                   reason     = excluded.reason,
                   revoked_at = datetime('now'),
                   revoked_by = excluded.revoked_by",
                params![hostname, not_before.to_rfc3339(), reason, revoked_by],
            )
            .context("upsert cert_revocations")?;
            Ok(())
        })
    }

    /// Caller compares against the presented cert's notBefore.
    pub fn cert_revoked_before(&self, hostname: &str) -> Result<Option<DateTime<Utc>>> {
        super::read(self.conn, |c| {
            match c.query_row(
                "SELECT not_before FROM cert_revocations WHERE hostname = ?1",
                params![hostname],
                |r| r.get::<_, String>(0),
            ) {
                Ok(s) => Ok(Some(
                    s.parse::<DateTime<Utc>>()
                        .context("parse revocation timestamp")?,
                )),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Reconcile the table with the signed sidecar: delete every row
    /// whose hostname is not in `keep`. Returns the number of rows
    /// deleted. Closes the de-revoke gap - `revoke_cert` is
    /// upsert-only, so an entry leaving the signed list otherwise
    /// sticks around in the DB forever and silently keeps rejecting
    /// the host on every mTLS request.
    pub fn retain_only(&self, keep: &[&str]) -> Result<usize> {
        super::read(self.conn, |c| {
            if keep.is_empty() {
                let n = c
                    .execute("DELETE FROM cert_revocations", [])
                    .context("clear cert_revocations")?;
                return Ok(n);
            }
            let placeholders = vec!["?"; keep.len()].join(",");
            let sql =
                format!("DELETE FROM cert_revocations WHERE hostname NOT IN ({placeholders})");
            let n = c
                .execute(&sql, rusqlite::params_from_iter(keep.iter()))
                .context("retain_only cert_revocations")?;
            Ok(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::fresh_db;
    use chrono::Utc;

    #[test]
    fn cert_revocation_upserts() {
        let db = fresh_db();
        assert!(
            db.revocations()
                .cert_revoked_before("test-host")
                .unwrap()
                .is_none()
        );
        let t1 = Utc::now();
        db.revocations()
            .revoke_cert("test-host", t1, Some("compromised"), Some("operator"))
            .unwrap();
        let r1 = db
            .revocations()
            .cert_revoked_before("test-host")
            .unwrap()
            .unwrap();
        // RFC3339 round-trip loses sub-second precision.
        assert_eq!(r1.timestamp(), t1.timestamp());
        let t2 = Utc::now() + chrono::Duration::seconds(60);
        db.revocations()
            .revoke_cert("test-host", t2, None, None)
            .unwrap();
        let r2 = db
            .revocations()
            .cert_revoked_before("test-host")
            .unwrap()
            .unwrap();
        assert!(r2 >= r1);
    }

    #[test]
    fn retain_only_deletes_absent_hostnames() {
        let db = fresh_db();
        let t = Utc::now();
        for host in ["alpha", "beta", "gamma"] {
            db.revocations()
                .revoke_cert(host, t, Some("test"), Some("operator"))
                .unwrap();
        }

        // Reconcile to a list with only beta - alpha + gamma should be gone.
        db.revocations().retain_only(&["beta"]).unwrap();
        assert!(
            db.revocations()
                .cert_revoked_before("alpha")
                .unwrap()
                .is_none()
        );
        assert!(
            db.revocations()
                .cert_revoked_before("beta")
                .unwrap()
                .is_some()
        );
        assert!(
            db.revocations()
                .cert_revoked_before("gamma")
                .unwrap()
                .is_none()
        );

        // Empty `keep` clears everything (operator wiped fleet.nix list).
        db.revocations().retain_only(&[]).unwrap();
        assert!(
            db.revocations()
                .cert_revoked_before("beta")
                .unwrap()
                .is_none()
        );
    }
}

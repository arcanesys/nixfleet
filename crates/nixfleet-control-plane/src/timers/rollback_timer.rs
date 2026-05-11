//! 30s sweep: past-deadline pending rows → 'rolled-back' + audit terminal stamp.
//!
//! LOADBEARING: CP marks state independently of the agent's local rollback
//! outcome - agent and CP halves converge through periodic checkin, not
//! through synchronous coupling here.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::state::TerminalState;

pub const ROLLBACK_TIMER_INTERVAL: Duration = Duration::from_secs(30);

pub fn spawn(cancel: CancellationToken, db: Arc<Db>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ROLLBACK_TIMER_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!(target: "shutdown", task = "rollback_timer", "task shut down");
                    return;
                }
                _ = ticker.tick() => {}
            }
            let expired = match db.host_dispatch_state().pending_deadlines() {
                Ok(rows) => rows,
                Err(err) => {
                    tracing::warn!(error = %err, "rollback timer: query failed");
                    continue;
                }
            };
            if expired.is_empty() {
                tracing::trace!("rollback timer: nothing expired");
                continue;
            }
            let now = Utc::now();
            let pairs: Vec<(String, String)> = expired
                .iter()
                .map(|(host, rollout, _, _)| (host.clone(), rollout.clone()))
                .collect();
            for (hostname, rollout_id, wave, target_closure) in &expired {
                tracing::info!(
                    target: "rollback",
                    hostname = %hostname,
                    rollout = %rollout_id,
                    wave,
                    target_closure = %target_closure,
                    "rolling back: confirm window expired"
                );
            }
            match db.host_dispatch_state().mark_rolled_back(&pairs) {
                Ok(n) => tracing::debug!(rolled_back = n, "rollback timer: operational marked"),
                Err(err) => tracing::warn!(error = %err, "rollback timer: operational mark failed"),
            }
            for (hostname, rollout_id, _, _) in &expired {
                if let Err(err) = db.dispatch_history().mark_terminal_for_rollout_host(
                    rollout_id,
                    hostname,
                    TerminalState::RolledBack,
                    now,
                ) {
                    tracing::warn!(
                        hostname = %hostname,
                        rollout = %rollout_id,
                        error = %err,
                        "rollback timer: audit terminal stamp failed",
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_expired_when_table_empty() {
        let db = Db::open_in_memory().unwrap();
        db.migrate().unwrap();
        let expired = db.host_dispatch_state().pending_deadlines().unwrap();
        assert!(expired.is_empty());
    }
}

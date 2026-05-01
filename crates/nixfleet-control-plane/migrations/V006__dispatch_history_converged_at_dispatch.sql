-- Add 'converged-at-dispatch' to dispatch_history.terminal_state's CHECK
-- constraint so the CP can stamp the dispatch_history row born-terminal
-- when a host enters a rollout already on-target. Without this the
-- atomic confirm txn that materialises a converged-at-dispatch host's
-- (operational + history + host_rollout_state) rows is rejected at
-- commit time, leaving the host's trace row `open` forever.
--
-- The terminal value is distinct from plain 'converged' so trace
-- consumers (fleet-status, dashboards) can show the operator that the
-- row never went through the activation lifecycle - it was born at the
-- terminal state.
--
-- SQLite doesn't support ALTER ... DROP CHECK; the only path is the
-- recreate-table dance:
--   1. Build a new table with the wider CHECK
--   2. Copy all rows over
--   3. Drop the old table
--   4. Rename
--   5. Recreate indexes (dropped with the old table)

CREATE TABLE dispatch_history_new (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname              TEXT NOT NULL,
    rollout_id            TEXT NOT NULL,
    channel               TEXT NOT NULL,
    wave                  INTEGER NOT NULL,
    target_closure_hash   TEXT NOT NULL,
    target_channel_ref    TEXT NOT NULL,
    dispatched_at         TEXT NOT NULL DEFAULT (datetime('now')),
    terminal_state        TEXT
        CHECK (terminal_state IN (
            'converged',
            'converged-at-dispatch',
            'rolled-back',
            'cancelled'
        )),
    terminal_at           TEXT
);

INSERT INTO dispatch_history_new
SELECT id, hostname, rollout_id, channel, wave,
       target_closure_hash, target_channel_ref,
       dispatched_at, terminal_state, terminal_at
FROM dispatch_history;

DROP TABLE dispatch_history;

ALTER TABLE dispatch_history_new RENAME TO dispatch_history;

CREATE INDEX dispatch_history_hostname_idx
    ON dispatch_history(hostname);
CREATE INDEX dispatch_history_rollout_idx
    ON dispatch_history(rollout_id);

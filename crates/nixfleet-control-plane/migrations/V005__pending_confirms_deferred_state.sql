-- Add 'deferred-pending-reboot' to host_dispatch_state.state's CHECK
-- constraint so the CP can park a dispatch that the agent has reported
-- as deferred to next-boot (issue #56). The deferred state means:
--   - profile is set on the host (`nix-env --set` ran)
--   - live `switch-to-configuration` was skipped because a critical
--     component (dbus impl, systemd, kernel, init) would have been
--     refused by `nixos-rebuild`'s switchInhibitors check
--   - new generation activates on next reboot (operator-paced)
--
-- The 360s confirm-deadline rollback timer must NOT sweep these rows
-- because the deferred lifecycle is human-paced, not agent-paced. The
-- existing `idx_host_dispatch_state_deadline` partial index is already
-- scoped to `WHERE state = 'pending'` so deferred rows naturally fall
-- out; this migration just extends the CHECK constraint to permit the
-- new value and recreates the indexes.
--
-- SQLite doesn't support ALTER ... DROP CHECK; the only path is the
-- recreate-table dance:
--   1. Build a new table with the wider CHECK
--   2. Copy all rows over
--   3. Drop the old table
--   4. Rename
--   5. Recreate indexes (dropped with the old table)

CREATE TABLE host_dispatch_state_new (
    hostname              TEXT PRIMARY KEY,
    rollout_id            TEXT NOT NULL,
    channel               TEXT NOT NULL,
    wave                  INTEGER NOT NULL,
    target_closure_hash   TEXT NOT NULL,
    target_channel_ref    TEXT NOT NULL,
    state                 TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN (
            'pending',
            'confirmed',
            'rolled-back',
            'cancelled',
            'deferred-pending-reboot'
        )),
    dispatched_at         TEXT NOT NULL DEFAULT (datetime('now')),
    confirm_deadline      TEXT NOT NULL,
    confirmed_at          TEXT
);

INSERT INTO host_dispatch_state_new
SELECT hostname, rollout_id, channel, wave,
       target_closure_hash, target_channel_ref,
       state, dispatched_at, confirm_deadline, confirmed_at
FROM host_dispatch_state;

DROP TABLE host_dispatch_state;

ALTER TABLE host_dispatch_state_new RENAME TO host_dispatch_state;

CREATE INDEX idx_host_dispatch_state_rollout
    ON host_dispatch_state(rollout_id);

-- Partial index, same shape as V001. The rollback timer reads through
-- this index; the deferred state is excluded by the partial WHERE so
-- the timer never sees deferred rows as expired.
CREATE INDEX idx_host_dispatch_state_deadline
    ON host_dispatch_state(confirm_deadline)
    WHERE state = 'pending';

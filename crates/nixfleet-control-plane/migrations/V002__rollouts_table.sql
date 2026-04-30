-- Per-rollout lifecycle state. Soft state - fully reconstructible after a
-- CP rebuild via channel-refs polling and on-dispatch inserts. Holds the
-- supersession chain so the active-rollouts panel and dispatch logic can
-- distinguish "still in flight" from "replaced by a newer rollout for the
-- same channel."
--
-- LOADBEARING: lazy population. Rows are inserted by record_active_rollout,
-- which is called from (a) channel-refs polling each tick and (b) dispatch
-- decisions on the checkin path. After a rebuild the table starts empty;
-- the first polling tick populates the current rollout per channel; agent
-- checkins fill in supersedes for any older rollouts they reference.
CREATE TABLE rollouts (
    rollout_id      TEXT PRIMARY KEY,
    channel         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    -- NULL while active. Set when a newer rollout for the same channel is
    -- recorded; the supersede UPDATE flips this in the same txn that
    -- inserts the newer row.
    superseded_at   TEXT,
    superseded_by   TEXT
);

-- Hot-path query: "is this rollout still active for its channel?" - used
-- by active_rollouts_snapshot's join and by the supersede UPDATE itself.
CREATE INDEX rollouts_channel_active_idx
    ON rollouts(channel) WHERE superseded_at IS NULL;

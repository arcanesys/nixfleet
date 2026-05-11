-- Mark a rollout as terminal once it has fully converged (all expected
-- hosts Soaked/Converged) OR the fleet snapshot moved on and the
-- rollout has no expected hosts in the current fleet.
--
-- Why this is needed: prior to this column, "active" was defined as
-- "row exists AND superseded_at IS NULL". A rollout converged on a
-- channel that subsequently lost all its hosts (or that simply
-- finished without a successor coming in to supersede it) stays
-- "active" forever. The reconciler emits ConvergeRollout actions on
-- every tick for these ghosts; downstream readers (deferrals view,
-- /v1/rollouts) keep showing them.
--
-- terminal_at is set by the reconciler when:
--   1. Action::ConvergeRollout fires (every host Soaked/Converged), OR
--   2. Per-tick orphan sweep finds a rollout whose channel no longer
--      has any expected hosts in the current fleet snapshot.
--
-- list_in_flight() filters both terminal_at AND superseded_at to give
-- a single, canonical "what's actually open" answer. Gate observed
-- builders read from there so a freshly-opened rollout (channelEdges
-- just released, no host dispatched yet) is visible to host-edges /
-- budget / compliance gates instead of looking like "no rollout"
-- (which collapsed input.rollout to None and disabled the gate).
ALTER TABLE rollouts ADD COLUMN terminal_at TEXT;

-- Hot-path: "is this rollout in flight?" - used by list_in_flight and
-- by the orphan sweep. Mirrors the existing rollouts_channel_active_idx
-- (intra-channel supersede) but covers the no-supersede-no-terminal
-- intersection more directly.
CREATE INDEX rollouts_in_flight_idx
    ON rollouts(channel)
    WHERE superseded_at IS NULL AND terminal_at IS NULL;

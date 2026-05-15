-- CP-side anti-thrash record. Inserted by the reconciler when the
-- sustained-health-failure sweep transitions a host Soaked -> Failed
-- under a rollback-and-halt policy (server/reconcile.rs::sweep_soaked_health_failures).
-- The reconciler refuses to emit DispatchHost for any rollout whose
-- (channel, target_closure_hash) appears here, so re-promoting the same
-- broken SHA does not thrash. Cleared lazily when the channel's declared
-- closure_hash moves past the quarantined SHA (observed_projection sees
-- the fleet has advanced and stamps cleared_at).

CREATE TABLE quarantined_closures (
    channel       TEXT NOT NULL,
    closure_hash  TEXT NOT NULL,
    reason        TEXT NOT NULL,
    quarantined_at TEXT NOT NULL DEFAULT (datetime('now')),
    cleared_at    TEXT,
    PRIMARY KEY (channel, closure_hash)
);

-- Read path: active quarantines for the gate.
CREATE INDEX idx_quarantined_closures_active
    ON quarantined_closures(channel, closure_hash)
    WHERE cleared_at IS NULL;

-- Release abstraction + rollout machinery.
--
-- Squashed form of the historical sequence:
--   V5 (rollouts + batches)   - pre-release-abstraction
--   V8 (rollout_events)       - timeline table added
--   V10 (releases)            - breaking migration that dropped+recreated
--                               rollouts/batches/events with release_id linkage
--
-- These were squashed into a single migration so a fresh DB lands directly
-- in the final shape without intermediate DROP+CREATE breadcrumbs.

-- Releases ------------------------------------------------------------------

CREATE TABLE releases (
    id          TEXT PRIMARY KEY,
    flake_ref   TEXT,
    flake_rev   TEXT,
    cache_url   TEXT,
    host_count  INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    created_by  TEXT NOT NULL
);

CREATE TABLE release_entries (
    release_id  TEXT NOT NULL REFERENCES releases(id),
    hostname    TEXT NOT NULL,
    store_path  TEXT NOT NULL,
    platform    TEXT NOT NULL,
    tags        TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (release_id, hostname)
);
CREATE INDEX idx_release_entries_release ON release_entries(release_id);

-- Rollouts (release_id -> per-host store paths) ----------------------------

CREATE TABLE rollouts (
    id                  TEXT PRIMARY KEY,
    release_id          TEXT NOT NULL REFERENCES releases(id),
    cache_url           TEXT,
    strategy            TEXT NOT NULL,
    batch_sizes         TEXT NOT NULL,
    failure_threshold   TEXT NOT NULL,
    on_failure          TEXT NOT NULL,
    health_timeout      INTEGER NOT NULL DEFAULT 300,
    status              TEXT NOT NULL DEFAULT 'created',
    target_tags         TEXT,
    target_hosts        TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    created_by          TEXT NOT NULL
);
CREATE INDEX idx_rollouts_status ON rollouts(status);

CREATE TABLE rollout_batches (
    id                      TEXT PRIMARY KEY,
    rollout_id              TEXT NOT NULL REFERENCES rollouts(id),
    batch_index             INTEGER NOT NULL,
    machine_ids             TEXT NOT NULL,
    status                  TEXT NOT NULL DEFAULT 'pending',
    started_at              TEXT,
    completed_at            TEXT,
    previous_generations    TEXT DEFAULT '{}'
);
CREATE INDEX idx_rollout_batches_rollout ON rollout_batches(rollout_id);

CREATE TABLE rollout_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    rollout_id  TEXT NOT NULL REFERENCES rollouts(id),
    event_type  TEXT NOT NULL,
    detail      TEXT NOT NULL DEFAULT '{}',
    actor       TEXT NOT NULL DEFAULT 'system',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_rollout_events_rollout ON rollout_events(rollout_id);

-- The `generations` table is KEPT (created in V1) - it is the mechanism
-- for the CP to communicate desired state to agents. The executor writes
-- per-host store paths to it when starting a batch.

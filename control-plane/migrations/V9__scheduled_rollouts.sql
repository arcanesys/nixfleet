-- Scheduled rollouts: one-shot deferred rollout creation.
CREATE TABLE scheduled_rollouts (
    id TEXT PRIMARY KEY,
    scheduled_at TEXT NOT NULL,
    policy_id TEXT REFERENCES rollout_policies(id),
    generation_hash TEXT NOT NULL,
    cache_url TEXT,
    strategy TEXT,
    batch_sizes TEXT,
    failure_threshold TEXT,
    on_failure TEXT,
    health_timeout_secs INTEGER,
    target_tags TEXT,
    target_hosts TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    rollout_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_by TEXT NOT NULL DEFAULT 'system'
);

CREATE INDEX idx_scheduled_rollouts_status ON scheduled_rollouts(status);

-- Add policy_id to rollouts table for traceability.
ALTER TABLE rollouts ADD COLUMN policy_id TEXT REFERENCES rollout_policies(id);

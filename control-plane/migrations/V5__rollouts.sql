CREATE TABLE rollouts (
    id TEXT PRIMARY KEY,
    generation_hash TEXT NOT NULL,
    cache_url TEXT,
    strategy TEXT NOT NULL,
    batch_sizes TEXT NOT NULL,
    failure_threshold TEXT NOT NULL,
    on_failure TEXT NOT NULL,
    health_timeout INTEGER NOT NULL DEFAULT 300,
    status TEXT NOT NULL DEFAULT 'created',
    target_tags TEXT,
    target_hosts TEXT,
    previous_generation TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    created_by TEXT NOT NULL
);
CREATE INDEX idx_rollouts_status ON rollouts(status);

CREATE TABLE rollout_batches (
    id TEXT PRIMARY KEY,
    rollout_id TEXT NOT NULL,
    batch_index INTEGER NOT NULL,
    machine_ids TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    started_at TEXT,
    completed_at TEXT,
    FOREIGN KEY (rollout_id) REFERENCES rollouts(id)
);
CREATE INDEX idx_rollout_batches_rollout ON rollout_batches(rollout_id);

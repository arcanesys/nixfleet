-- Rollout policies: named, reusable rollout configuration presets.
CREATE TABLE rollout_policies (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    strategy TEXT NOT NULL,
    batch_sizes TEXT NOT NULL DEFAULT '["100%"]',
    failure_threshold TEXT NOT NULL DEFAULT '1',
    on_failure TEXT NOT NULL DEFAULT 'pause',
    health_timeout_secs INTEGER NOT NULL DEFAULT 300,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

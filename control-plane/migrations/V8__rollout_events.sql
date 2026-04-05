-- Rollout events: state transition log for rollout history/timeline.
CREATE TABLE rollout_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rollout_id TEXT NOT NULL REFERENCES rollouts(id),
    event_type TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '{}',
    actor TEXT NOT NULL DEFAULT 'system',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_rollout_events_rollout ON rollout_events(rollout_id);

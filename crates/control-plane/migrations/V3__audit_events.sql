CREATE TABLE IF NOT EXISTS audit_events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT    NOT NULL DEFAULT (datetime('now')),
    actor     TEXT    NOT NULL,
    action    TEXT    NOT NULL,
    target    TEXT    NOT NULL,
    detail    TEXT
);

CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_events(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_events(actor);

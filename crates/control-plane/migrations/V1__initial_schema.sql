CREATE TABLE IF NOT EXISTS generations (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id TEXT    NOT NULL,
    hash       TEXT    NOT NULL,
    set_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(machine_id)
);

CREATE TABLE IF NOT EXISTS reports (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id    TEXT    NOT NULL,
    generation    TEXT    NOT NULL,
    success       INTEGER NOT NULL,
    message       TEXT,
    received_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_reports_machine
    ON reports(machine_id, received_at DESC);

CREATE TABLE IF NOT EXISTS machines (
    machine_id    TEXT PRIMARY KEY,
    lifecycle     TEXT NOT NULL DEFAULT 'active',
    registered_at TEXT NOT NULL DEFAULT (datetime('now'))
);

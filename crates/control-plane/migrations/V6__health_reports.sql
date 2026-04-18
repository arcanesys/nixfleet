CREATE TABLE health_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id TEXT NOT NULL,
    results TEXT NOT NULL,
    all_passed INTEGER NOT NULL,
    received_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_health_reports_machine ON health_reports(machine_id, received_at);

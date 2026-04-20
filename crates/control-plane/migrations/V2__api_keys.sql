CREATE TABLE IF NOT EXISTS api_keys (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    key_hash   TEXT    NOT NULL UNIQUE,
    name       TEXT    NOT NULL,
    role       TEXT    NOT NULL DEFAULT 'readonly',
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    CHECK(role IN ('readonly', 'deploy', 'admin'))
);

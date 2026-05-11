-- Consolidated nixfleet-control-plane schema.
--
-- This file is the single migration any fresh CP applies on first
-- boot. Earlier development applied V1 + V002-V007 incrementally;
-- once production was wiped (single CP, single fleet), the history
-- collapsed into this clean shape. Adding a new schema change goes
-- in V002__<name>.sql and gets a per-migration test in db/mod.rs
-- alongside the V001 baseline test.
--
-- Six tables:
--   token_replay         - bootstrap-token nonce replay defence
--                          (24h TTL pruned by prune_timer)
--   cert_revocations     - agent cert revocation list, replayed on
--                          every reconcile tick from the signed
--                          revocations.json sidecar
--   host_rollout_state   - per-(host, rollout) state-machine entry
--                          for the soak / converge / failed pipeline
--                          (RFC-0002 §3.2)
--   host_reports         - durable per-host event log; backs the
--                          in-memory ring used by the runtime gate
--   host_dispatch_state  - operational "what is host X doing right
--                          now" (one row per host; UPSERTed each
--                          dispatch; terminal states stay parked
--                          until the next dispatch overwrites them)
--   dispatch_history     - append-only audit log; one row per
--                          dispatch event, terminal_state stamped
--                          on completion, pruned by 90d retention

CREATE TABLE token_replay (
    nonce       TEXT PRIMARY KEY,
    hostname    TEXT NOT NULL,
    first_seen  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_token_replay_first_seen
    ON token_replay(first_seen);

CREATE TABLE cert_revocations (
    hostname     TEXT PRIMARY KEY,
    not_before   TEXT NOT NULL,
    reason       TEXT,
    revoked_at   TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_by   TEXT
);

CREATE TABLE host_rollout_state (
    rollout_id          TEXT NOT NULL,
    hostname            TEXT NOT NULL,
    host_state          TEXT NOT NULL DEFAULT 'Dispatched'
        CHECK (host_state IN ('Queued', 'Dispatched', 'Activating',
                              'ConfirmWindow', 'Healthy', 'Soaked',
                              'Converged', 'Reverted', 'Failed')),
    last_healthy_since  TEXT,
    updated_at          TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (rollout_id, hostname)
);

CREATE INDEX idx_host_rollout_state_hostname
    ON host_rollout_state(hostname);

CREATE TABLE host_reports (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname           TEXT NOT NULL,
    event_id           TEXT NOT NULL UNIQUE,
    received_at        TEXT NOT NULL,                  -- RFC3339 UTC
    event_kind         TEXT NOT NULL,                  -- ReportEvent kebab-case discriminator
    rollout            TEXT,                           -- nullable; matches ReportRequest.rollout
    signature_status   TEXT,                           -- kebab-case SignatureStatus, NULL for non-signed events
    report_json        TEXT NOT NULL                   -- full ReportRequest envelope
);

CREATE INDEX idx_host_reports_hostname ON host_reports(hostname);
CREATE INDEX idx_host_reports_received ON host_reports(received_at);

CREATE TABLE host_dispatch_state (
    hostname              TEXT PRIMARY KEY,
    rollout_id            TEXT NOT NULL,
    channel               TEXT NOT NULL,
    wave                  INTEGER NOT NULL,
    target_closure_hash   TEXT NOT NULL,
    target_channel_ref    TEXT NOT NULL,
    state                 TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'confirmed', 'rolled-back', 'cancelled')),
    dispatched_at         TEXT NOT NULL DEFAULT (datetime('now')),
    confirm_deadline      TEXT NOT NULL,
    confirmed_at          TEXT
);

CREATE INDEX idx_host_dispatch_state_rollout
    ON host_dispatch_state(rollout_id);

-- Partial index for the magic-rollback timer's deadline scan.
CREATE INDEX idx_host_dispatch_state_deadline
    ON host_dispatch_state(confirm_deadline)
    WHERE state = 'pending';

CREATE TABLE dispatch_history (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    hostname              TEXT NOT NULL,
    rollout_id            TEXT NOT NULL,
    channel               TEXT NOT NULL,
    wave                  INTEGER NOT NULL,
    target_closure_hash   TEXT NOT NULL,
    target_channel_ref    TEXT NOT NULL,
    dispatched_at         TEXT NOT NULL DEFAULT (datetime('now')),
    terminal_state        TEXT
        CHECK (terminal_state IN ('converged', 'rolled-back', 'cancelled')),
    terminal_at           TEXT
);

CREATE INDEX dispatch_history_hostname_idx
    ON dispatch_history(hostname);
CREATE INDEX dispatch_history_rollout_idx
    ON dispatch_history(rollout_id);
CREATE INDEX dispatch_history_dispatched_idx
    ON dispatch_history(dispatched_at);

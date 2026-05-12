# nixfleet-control-plane

**Role.** TLS server plus reconciler driver. Wraps `nixfleet-reconciler`'s pure decision procedure in an Axum HTTP service backed by SQLite operational state, polls the signed-artefact directory for fresh `fleet.resolved.json`, projects an `Observed` view from the database, dispatches `Action`s by emitting agent-facing wire bodies, and persists everything per ADR-012 (fire-and-forget dispatch with idempotent retries). Carries the routing-and-state-storage half of the agent / CP protocol; the agent never talks to anything else.

**Key types and state machines.** `TickInputs` / `TickOutput` / `VerifyOutcome` / `VerifyOk` (one reconciler iteration's input bundle and result), `AppState` (server-wide handle wiring `Mutex<rusqlite::Connection>` sized for O(100) hosts, the verified-fleet snapshot, the signed-artefact poller, the freshness window, the optional Prometheus registry), the polling timer (re-verifies the on-disk artefact at `signing_interval_minutes` cadence), the rollouts-source (resolves a manifest into the per-channel dispatch plan). Persisted state machines mirror `nixfleet-reconciler`: `HostRolloutState`, `RolloutState`, plus the `dispatch_history` log used by `/v1/rollouts/{id}/trace`.

**Surface.** Library: `tick(&TickInputs) -> Result<TickOutput>` (one reconciliation iteration) and `render_plan(&TickOutput) -> String` (JSONL summary - one tick line plus one line per action, with offline `Skip`s coalesced into a `skip_summary`). Binary: HTTP routes under `/v1/agent/*` (`checkin`, `confirm`, `report`, `renew`), `/v1/enroll`, `/v1/whoami`, `/v1/hosts`, `/v1/host-reports`, `/v1/channels/{name}`, `/v1/rollouts`, `/v1/rollouts/{id}`, `/v1/rollouts/{id}/trace`, `/v1/deferrals`, `/metrics`, `/healthz`. NixOS module `services.nixfleet-control-plane.{enable, listen, tls.{caCert, certFile, keyFile}, trustFile, ...}` materialises the `nixfleet-control-plane.service` systemd unit.

**Links.**

- Generated rustdoc: [`api/nixfleet_control_plane/`](../../api/nixfleet_control_plane/index.html)
- Relevant RFCs: [RFC-0002](../../rfcs/0002-reconciler.md), [RFC-0003](../../rfcs/0003-protocol.md), [RFC-0006](../../rfcs/0006-freshness-window-policy.md)
- Architecture component: [§1.4 Control plane](../../design/architecture.md#14-control-plane-the-router), [§3 The main flow](../../design/architecture.md#3-the-main-flow)

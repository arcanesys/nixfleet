# Phase 3 entry spec

Sequences RFC-0003 (agent ↔ control-plane wire) into five reviewable PRs. End deliverable: each NixOS fleet host runs a real `nixfleet-agent` that polls the CP over mTLS and reports its current generation. The CP records check-ins, derives `Observed` state from them (replacing Phase 2's hand-written `observed.json`), and the reconciler plan reflects actual fleet state. **No activation runs** — that's Phase 4.

Cross-references: `docs/KICKOFF.md` §1 Phase 3, `rfcs/0003-protocol.md` (the wire spec), `rfcs/0002-reconciler.md` §4 (the reconciler this feeds), `docs/trust-root-flow.md` §3 (the trust-file pipeline).

Status: **proposed** — adopt as the implementation plan for the next ~5 PRs.

## 1. Goal

Phase 2 made the reconciler real on lab as a oneshot timer reading a hand-written `observed.json`. Phase 3 turns the same binary into a long-running TLS server with one new internal loop and four new HTTP endpoints, and adds a real `nixfleet-agent` body that talks to it. By the end of Phase 3:

- Lab's CP listens on a TLS port, accepting mTLS-authenticated agent connections.
- Each NixOS fleet host runs `nixfleet-agent` as a systemd service. It POSTs `/v1/agent/checkin` every 60s with its current closure hash + bootId.
- The CP's reconcile loop ticks every 30s on an in-memory `Observed` derived from check-ins (the file-backed `--observed` becomes a dev/test fallback).
- Adding a new fleet host = declare in `fleet.nix` + agenix-encrypted bootstrap token; first boot self-enrols and immediately begins checking in.

**Not in Phase 3** (deferred to later phases, even though some live in RFC-0003):

- Activation (`nixos-rebuild switch` from agent) — Phase 4.
- Magic rollback (`/v1/agent/confirm` deadline) — Phase 4 (only meaningful once activation can fail).
- Probe execution + signed evidence — Phase 7.
- Compliance gates as rollout blockers — Phase 6/7.
- Cert rotation cadence (`/v1/agent/renew`) — Phase 4 polish.
- Failure-event reports (`/v1/agent/report`) — Phase 4 (only meaningful once activation can fail).
- Closure proxy (`/v1/agent/closure/<hash>`) — Phase 4 (only relevant when agents fetch closures).
- Darwin (`aether`) agent support — non-goal; stays manually managed until Phase 5+.

## 2. The architectural shift

Phase 2 was:

```
systemd timer (5min) ──▶ nixfleet-control-plane (oneshot)
                            ├── verify_artifact(fleet.resolved)
                            ├── reconcile(observed.json)
                            └── emit JSON-line plan to journal, exit
```

Phase 3 becomes:

```
systemd service (long-running) ──▶ nixfleet-control-plane (server)
                                     ├── mTLS listener (PR-1, PR-2)
                                     ├── GET  /healthz                  (PR-1)
                                     ├── GET  /v1/whoami                (PR-2)
                                     ├── POST /v1/agent/checkin         (PR-3)
                                     ├── POST /v1/enroll                (PR-5)
                                     ├── tokio::time::interval(30s) ──▶ reconcile(in-memory Observed)
                                     └── emit JSON-line plan to journal each tick

                          systemd service (per host) ──▶ nixfleet-agent
                                                          ├── tokio + reqwest mTLS
                                                          ├── poll /v1/agent/checkin every 60s
                                                          └── log target on stdout
```

The shift is real: the reconciler stops being a side-effect-free CLI and becomes a co-located function inside the server's tick loop. PR #36 deliberately deferred this — PR-1 below is where it lands.

## 3. PR breakdown

### PR-1 — CP becomes a long-running TLS server with `/healthz`

**Scope.** Restructure `nixfleet-control-plane` from oneshot to long-running. One real endpoint (`GET /healthz`) for proof-of-life. TLS-only listener (server cert + key from operator-supplied paths). mTLS not required yet — PR-2 adds it.

**Concrete.**

- Re-add to `crates/nixfleet-control-plane/Cargo.toml`: `tokio` (full), `axum 0.8`, `axum-server 0.7` (tls-rustls), `rustls 0.23`. (Removed in PR #36 as Phase 2 didn't need them.)
- Subcommand split: `nixfleet-control-plane serve` (long-running) and `nixfleet-control-plane tick` (oneshot, kept for tests + ad-hoc operator runs). Default subcommand is `serve`.
- New `src/server.rs` with axum router + axum-server TLS listener.
- Internal reconcile loop: `tokio::time::interval(Duration::from_secs(30))` calls the existing `tick()` function and emits the plan via tracing.
- NixOS module switches from oneshot+timer to a `simple` always-running service. Re-add `--listen` and `--tls-cert/--tls-key` options. Keep `--observed` as fallback for the file-backed input until PR-4.
- `/healthz` returns `{"ok": true, "version": "<crate version>", "lastTickAt": "<rfc3339>"}`.
- Tests: rcgen-generated server cert, hit `/healthz` with reqwest, assert 200 + valid JSON.

**Deliverable.** From the operator's workstation:

```
curl --cacert /etc/nixfleet/fleet-ca.pem https://lab:8080/healthz
# {"ok":true,"version":"0.2.0","lastTickAt":"2026-04-25T12:34:56Z"}
```

`journalctl -u nixfleet-control-plane.service` on lab shows reconcile-tick JSON lines every 30s.

**Open decisions.** §8 D1, D2, D3.

### PR-2 — mTLS + `/v1/whoami`

**Scope.** Server requires a verified client cert; verifies against an operator-supplied CA. Adds `GET /v1/whoami` returning the verified CN of the client — useful for confirming the cert pipeline before the agent body is real.

**Concrete.**

- axum-server TLS config: `ClientCertVerifier` against the configured CA.
- `/healthz` remains unauthenticated (operational debuggability — see §8 D7); `/v1/*` requires verified mTLS.
- Extract client CN via `x509-parser` on the verified cert chain.
- `/v1/whoami` returns `{"cn": "<client-CN>", "issuedAt": "<rfc3339>"}`.
- NixOS module re-adds `--client-ca` flag (the v0.2 skeleton had it).
- Tests: rcgen generates server cert, valid client cert, invalid client cert. Verify whoami returns CN for valid; rejected (TLS handshake failure) for invalid.

**Deliverable.** From any fleet host:

```
curl --cert /run/agenix/agent-krach-cert \
     --key  /run/agenix/agent-krach-key \
     --cacert /etc/nixfleet/fleet-ca.pem \
     https://lab:8080/v1/whoami
# {"cn":"krach","issuedAt":"..."}
```

### PR-3 — Agent body: first `/v1/agent/checkin`

**Scope.** Replace the `tracing::info!` skeleton in `nixfleet-agent` with a real poll loop. Send `/v1/agent/checkin` every `pollInterval` seconds. CP records the check-in into in-memory state and responds with `target: null` (no rollouts dispatched in Phase 3 — that's Phase 4).

**Concrete.**

- `nixfleet-agent`: real main loop. Reads cert paths from CLI args (already present in module). Builds a `reqwest::Client` with mTLS. Polls `/v1/agent/checkin` every 60s.
- Checkin request body per RFC-0003 §4.1:
  ```json
  {
    "hostname": "krach",
    "agentVersion": "0.2.0",
    "currentGeneration": {
      "closureHash": "<hash from /run/current-system>",
      "channelRef": null,
      "bootId": "<from /proc/sys/kernel/random/boot_id>"
    }
  }
  ```
- CP-side: `POST /v1/agent/checkin` handler. Validates the verified mTLS CN matches the body's `hostname` (sanity check, not a security boundary — mTLS already authenticated). Records into `Arc<RwLock<HashMap<String, HostState>>>`. Returns `{"target": null, "nextCheckinSecs": 60}`.
- Tests:
  - Cargo integration test: spin up CP in-process (axum), agent in-process, run one check-in, assert state captured.
  - Optional: extend the PR #34 harness scenario to make two agent microVMs check in to a host CP — `journalctl -u nixfleet-control-plane | grep checkin` shows both hostnames within 60s. (May land as PR-3.5 if it grows.)

**Deliverable.** `journalctl -u nixfleet-control-plane.service` on lab shows entries like `checkin received hostname=krach closureHash=861d2y2zmssij…`. Each fleet host's `journalctl -u nixfleet-agent` shows successful periodic checkins.

### PR-4 — Live `Observed` projection from check-ins

**Scope.** CP derives `Observed` (the existing `nixfleet_reconciler::Observed` input type) from in-memory check-in state. Reconcile loop reads from this projection instead of `observed.json`. The hand-written file becomes opt-in via `--observed` flag for tests/dev only.

**Concrete.**

- New module `src/observed_projection.rs`: takes the in-memory `HashMap<String, HostState>` plus a configured `channel_refs` source (per §8 D4) and produces an `Observed`.
- Server's reconcile loop calls `project()` each tick, then `reconcile()`.
- Plan JSON-line format unchanged from Phase 2.
- The `--observed` flag stays — useful for offline-replay debugging (operator dumps in-memory state to a file, reproduces a tick).
- Tests: simulated check-ins → projection → reconcile → assert plan reflects reported state.

**Deliverable.** Operator commits a no-op release commit. CI signs. Workstations auto-upgrade (per fleet PR #47) to that commit. Each host checks in with its new closure hash. Lab's reconcile loop sees the converged state and emits zero actions. *Diverged* state would emit `OpenRollout` (the Phase 4 dispatch loop is what would then act on it).

**Open decisions.** §8 D4 — channel-ref source for the projection.

### PR-5 — Bootstrap enrollment

**Scope.** `POST /v1/enroll` accepts a CSR + bootstrap token; verifies the token against the org root key; issues a 30-day client cert signed by the fleet CA. Agent has a one-shot enrollment mode for first boot when no cert exists.

**Concrete.**

- Org root key bootstrap, parallel to the `ciReleaseKey` TPM bootstrap from fleet PR #45:
  - Generate ed25519 keypair offline (operator workstation per §8 D5; later PRs may move to Yubikey).
  - Declare pubkey under `nixfleet.trust.orgRootKey.current` in `fleet/modules/nixfleet/trust.nix`.
  - Private key kept on operator workstation (or a Yubikey when §8 D5 is upgraded).
- New tiny binary `nixfleet-mint-token` in `crates/nixfleet-cli` (or a new crate): operator runs `nixfleet-mint-token --hostname krach --csr-pubkey-fingerprint <sha256>` once per host before first deploy; emits a one-shot token signed with the org root private key.
- CP-side `/v1/enroll` handler:
  - Verify token signature against `orgRootKey.current` from trust.json.
  - Verify token's `expectedHostname` matches the CSR's CN.
  - Verify token's `expectedPubkeyFingerprint` matches the CSR's public key.
  - Verify token hasn't been used (in-memory replay set; persistence is Phase 4).
  - Issue a cert (signed by the fleet CA — same CA the existing per-host certs use).
- Agent-side first-boot mode:
  - On startup, if `--cert/--key` files don't exist (or cert is expired), enter enrollment.
  - Read `--bootstrap-token` path. Generate a CSR (`rcgen`). POST `/v1/enroll`. Write returned cert to disk.
  - Resume normal checkin loop.
- Module updates:
  - Agent module gains `bootstrapTokenFile` option.
  - Fleet-secrets gains `bootstrap-token-${hostname}` agenix entries (operator generates + commits per host).
- Tests:
  - End-to-end enroll → checkin happy path (cargo integration test).
  - Token replay rejected.
  - Tampered token rejected.

**Deliverable.** Adding a new fleet host:
1. Declare in `fleet.nix`.
2. Operator runs `nixfleet-mint-token --hostname <new-host> ...`, agenix-encrypts the result.
3. First boot: agent enrols, immediately begins checking in.

No manual SSH-to-lab-and-copy-cert step.

**Open decisions.** §8 D5, D6.

## 4. Test substrate

The PR #34 signed-roundtrip harness scenario is the substrate. Phase 3 PRs extend it:

- **PR-1**: cargo integration test only (binary smoke); harness untouched.
- **PR-2**: cargo integration test for mTLS handshake (rcgen-based cert generation in-test).
- **PR-3**: extend the harness scenario to make agent microVMs check in to the host CP. Replaces the curl+verify-artifact wrapper with the real agent binary. (May land as PR-3.5 if it grows.)
- **PR-4**: extend the harness assertion to grep for `checkin received` in CP journal across multiple agents.
- **PR-5**: new harness scenario `fleet-harness-enroll-checkin`: agent boots without a cert, has a bootstrap token, enrols, then checks in.

## 5. Cargo dep changes per PR

| PR | Adds to `nixfleet-control-plane` | Adds to `nixfleet-agent` |
|---|---|---|
| PR-1 | tokio (full), axum, axum-server (tls-rustls), rustls — re-add | — |
| PR-2 | x509-parser, rustls-pki-types | — |
| PR-3 | (no new server-side) | tokio, reqwest (rustls-tls-native-roots), serde_json |
| PR-4 | (no new — projection is pure logic) | — |
| PR-5 | rcgen, sha2, hex (token signing/verification primitives) | rcgen (CSR generation) |

## 6. Hard prerequisites before PR-1

These need to be true on lab before PR-1 can ship:

1. **CP server cert + key in agenix.** Dropped from `fleet/modules/nixfleet/tls.nix` in PR #46 to unblock the Phase 2 deploy; need to come back. Specifically: declare `cp-cert` and `cp-key` secrets in `fleet-secrets`, encrypt to lab's pubkey, re-add the wiring.
2. **Fleet CA exists at `_config/fleet-ca.pem`.** The agent TLS block already references it (see `fleet/modules/nixfleet/tls.nix`); verify the file is committed and the corresponding private key is offline somewhere (used to sign agent + CP server certs).
3. **Per-host agent certs in agenix.** `agent-${hostName}-{cert,key}` already declared per `fleet/modules/secrets/nixos.nix`; verify they're populated for `krach`, `ohm`, `lab`, `pixel` (aether deferred).

If these don't exist, PR-1 is blocked on a fleet-side prep PR that creates them. Estimate: ~1h (key generation + agenix encryption + wiring re-add).

## 7. Order

Strictly sequential: PR-1 → PR-2 → PR-3 → PR-4 → PR-5. Each PR is shippable on its own and unblocks the next.

Rough size estimates:

| PR | Rust LOC | NixOS LOC | Effort (focused) |
|---|---|---|---|
| Prep | — | ~50 | ~1h |
| PR-1 | ~400 | ~50 | half-day |
| PR-2 | ~150 | ~20 | few hours |
| PR-3 | ~500 | ~50 | full day |
| PR-4 | ~200 | ~20 | half-day |
| PR-5 | ~700 | ~80 | 1-2 days |

Total Phase 3: ~3-4 days focused, ~1-2 weeks part-time.

Phase 4 (activation + magic rollback) layers on top — that's where the agent gains `nixos-rebuild switch`, the CP gains dispatch + soak + rollback semantics, and `system.autoUpgrade` on workstations (fleet PR #47) gets disabled per-host as the agent supersedes it.

## 8. Decisions to lock in before PR-1

Confirm before implementation starts. **Defaults stand if you don't override.**

### D1 — CP server cert source (Phase 3 prep)

**Default.** Re-add `cp-cert/cp-key` to `fleet-secrets` as agenix-encrypted secrets, mirroring how `agent-${hostName}-cert/key` already work. Same fleet CA signs both. Operator generates the keypair offline once, encrypts to lab's pubkey, commits.

**Alternative.** Self-signed cert generated at first boot. Simpler bootstrap, harder rotation, fights the architecture's "everything is signed by something offline" principle.

### D2 — Reconcile cadence

**Default.** 30s. Fast enough that operator-visible drift (host failed to check in) shows up in the journal within one cycle; slow enough not to spam the journal.

**Alternative.** 60s (matches RFC-0003 default polling); 10s (tighter operator feedback at the cost of journal noise).

### D3 — Server port

**Default.** 8080 (HTTPS). Matches the v0.2 skeleton; `ports < 1024` would require CAP_NET_BIND_SERVICE; 443 collides with operator-facing services on lab.

**Alternative.** A non-standard port (8443? 9443?) for less collision-prone discoverability. No strong reason.

### D4 — Channel-ref source for the in-memory projection (PR-4)

**Default.** Hand-edited `/etc/nixfleet/cp/channel-refs.json`, declared by the CP NixOS module. Operator updates after each release. Phase 4 introduces auto-discovery (CP polls forgejo or git on disk).

**Alternative.** CP polls forgejo's API in-process. Adds an HTTP client + auth concern in Phase 3; defer.

### D5 — Org root private key (PR-5)

**Default.** File on operator workstation, consumed by `nixfleet-mint-token`. Simplest bootstrap; rotation is a documented procedure.

**Alternative.** Yubikey-resident from day one. Right end-state per the architecture doc; adds hardware setup steps before PR-5 can land. Fine to defer to Phase 9 polish.

### D6 — Cert validity (PR-5)

**Default.** 30d, matching RFC-0003 §2 ("agent requests renewal at 50% of remaining validity"). But **no `/v1/agent/renew` endpoint in Phase 3** — agent re-enrolls on expiry until Phase 4 adds renewal. Operator regenerates bootstrap tokens periodically, or manually re-issues certs.

**Alternative.** Longer (1y) for Phase 3 only; switch to 30d when `/v1/agent/renew` lands. Reduces Phase 3 ops toil.

### D7 — `/healthz` authentication

**Default.** Unauthenticated. Operational debuggability (curl from anywhere with network reachability + CA trust) outweighs the marginal sovereignty gain of mTLS-gating a status endpoint.

**Alternative.** mTLS-required like `/v1/*`. Strict default; reachable only from agent-equipped hosts.

### D8 — Phase 3 scope: include `/v1/agent/{confirm,report}` stubs?

**Default.** Defer. Both endpoints are only meaningful once activation can fail (Phase 4). Stubbing them now bakes a wire shape that may need to change.

**Alternative.** Land 410-Gone stubs in PR-3 so the surface URL exists and clients see a deterministic error. Marginal benefit.

---

When you've confirmed (or pushed back on) the decisions above, PR-1 can start. The prep PR for the CP server cert (§6 #1) goes first.

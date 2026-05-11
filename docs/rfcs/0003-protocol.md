# RFC-0003: Agent ↔ control-plane protocol

**Status.** Accepted.
**Depends on.** RFC-0001, RFC-0002, nixfleet #2 (magic rollback).
**Scope.** Wire protocol between agent and control plane. Identity, endpoints, polling, versioning, security properties. Does not cover control-plane-internal APIs.
**Implementation.** `crates/nixfleet-proto/src/agent_wire.rs` + `enroll_wire.rs` (request/response types), `crates/nixfleet-control-plane/src/server/` (the `/v1/*` handlers and mTLS gate), `crates/nixfleet-agent/` (the poll loop, enrollment, magic-rollback timer). See `ARCHITECTURE.md` §1.4 / §1.5 / §6 Phase 3.

## 1. Design goals

1. **Pull-only for control flow.** Agents initiate every connection. Control plane never needs to reach an agent - works behind CGNAT, hotel WiFi, intermittent links.
2. **Stateless on the wire.** Each request is self-describing. No sessions, no long-lived connections, no WebSockets in v1.
3. **Declarative intent, not commands.** The control plane answers "what should host X be running?", never "run this command". Scripted execution is outside the agent's vocabulary on purpose.
4. **Zero-knowledge for secrets.** Secrets do not transit the control plane in plaintext (see nixfleet #6). The protocol carries closure hashes and references, not secret material.
5. **Explicitly versioned.** Every request and response carries a protocol version. Mismatches fail loudly.

## 2. Identity model

- **Host key = SSH host ed25519 key.** Machine-lifetime key already present on every NixOS host (`/etc/ssh/ssh_host_ed25519_key`). Signs probe outputs (RFC-0002 §5.3), decrypts agenix secrets, anchors the agent's cryptographic identity. Not transmitted to the control plane; only its public half is declared in `fleet.nix`.
- **Agent identity = mTLS client certificate, derived from the host key.** At enrollment (nixfleet #9), the agent generates the CSR using the SSH host key as the signing key; the public key in the cert is the host's SSH public key. CN = `hostname`, SANs carry declared host attributes (channel, tags - redundant with fleet.resolved, used only for sanity checking). This binding means compromising the mTLS cert and compromising the host key are the same event; short-lived certs bound the exposure of that event.
- **Cert issuance.** Agent sends the CSR + a one-shot bootstrap token (signed by the org root key, scoped to `expectedHostname` + `expectedPubkeyFingerprint`). Control plane verifies both, issues cert with 30-day validity. A mismatch between the CSR's public key and the token's `expectedPubkeyFingerprint` aborts enrollment.
- **Cert rotation.** Agent requests renewal at 50% of remaining validity. Old cert valid until expiry; overlap prevents downtime.
- **Cert revocation.** Control plane maintains a small revocation set (hostname → notBefore timestamp). Agents with certs issued before `notBefore` for their hostname are rejected. Simpler than CRLs; works because cert lifetime is short.
- **No shared credentials.** No API keys, no HMAC secrets, no bearer tokens. mTLS end to end.

## 3. Wire format

- **Transport.** HTTP/2 over TLS 1.3. mTLS mandatory.
- **Body.** JSON. Canonical field names, no nulls (absence means absence), timestamps RFC 3339 UTC.
- **Headers.**
  - `X-Nixfleet-Protocol: 1` - major version. Mismatched = 400.
  - `X-Nixfleet-Agent-Version: <semver>` - informational.
  - `Content-Type: application/json`.
- **Why not gRPC/protobuf?** Stability, debuggability, homelab introspection. Revisit if wire size becomes a problem (it won't at fleet sizes nixfleet targets).

## 4. Endpoints

All endpoints rooted at `https://<control-plane>/v1/`.

### 4.1 `POST /agent/checkin`

The core of the protocol. Agent polls this on its declared interval.

**Request body:**

```json
{
  "hostname": "m70q-attic",
  "agentVersion": "0.2.1",
  "currentGeneration": {
    "closureHash": "sha256-aabbcc...",
    "channelRef": "abc123def",
    "bootId": "f0e1d2c3-..."
  },
  "health": {
    "systemdFailedUnits": [],
    "uptime": 1234567,
    "loadAverage": [0.1, 0.2, 0.3]
  },
  "lastProbeResults": [
    { "control": "anssi-bp028-ssh-no-password", "status": "passed",
      "evidence": "...", "ts": "2026-04-24T10:15:02Z" }
  ]
}
```

**Response body:**

```json
{
  "target": {
    "closureHash": "sha256-ddeeff...",
    "channelRef": "def456abc",
    "rollout": "a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1",
    "wave": 2,
    "activate": {
      "confirmWindowSecs": 120,
      "confirmEndpoint": "/v1/agent/confirm",
      "runtimeProbes": [
        { "control": "anssi-bp028-ssh-no-password", "schema": "anssi-bp028/v1" }
      ]
    }
  },
  "nextCheckinSecs": 60
}
```

If the host is already at the desired generation, `target` is absent and `nextCheckinSecs` reflects idle polling.

**`target.rollout` is a content hash.** It is the SHA-256 (hex, lowercase) of the canonical bytes of the rollout's `RolloutManifest` (see RFC-0002 §4.4). It is NOT a human-readable label - the human-readable `<channel>@<short-ci-commit>` annotation lives inside the manifest as `displayName`. Operator surfaces (`nixfleet status`, log lines) MAY display a short prefix paired with the annotation, but the wire field carries the full hex.

**Agent verification posture (mandatory).** On first sight of a `rolloutId` it has not seen before, the agent MUST:

1. Fetch `GET /v1/rollouts/<rolloutId>` and the matching `.sig` (§4.6).
2. Verify the signature against the trust roots it already holds (`ciReleaseKey`).
3. Recompute `sha256(canonical(manifest))` and assert it equals the advertised `rolloutId`.
4. Assert `(hostname, wave_index)` ∈ `manifest.host_set`.
5. Cache the manifest bytes + signature under `<state-dir>/rollouts/<rolloutId>.{json,sig}`.

Failure of any step is a hard refuse-to-act: the agent emits the corresponding `ReportEvent` (`ManifestMissing`, `ManifestVerifyFailed`, `ManifestMismatch`) and does not consume any other field of `target`. There is no fallback path that trusts a CP-advertised `target` for one tick - see RFC-0002 §4.4 for the threat model this closes.

**Subsequent checkins.** For every checkin where `target.rollout` matches a `rolloutId` the agent has already cached, the agent MUST re-assert string equality against the cache. A change in `rolloutId` while the cached one is still in flight is a hard refuse-to-act; the CP cannot replace a rollout's plan mid-flight by content-address (a different plan is a different rollout).

**Idempotency.** Repeated check-ins from the same host with unchanged state are no-ops (but still update `lastSeen` for observability). The control plane must not create duplicate work.

### 4.2 `POST /agent/confirm`

Called exactly once by the agent, after a new generation has booted and the agent process has come up healthy. The magic-rollback window (nixfleet #2) closes on receipt.

**Request body:**

```json
{
  "hostname": "m70q-attic",
  "rollout": "a3f7c2b1d4e8f6a9c0b5d2e7f1a4c8b3d6e9f2a5c7b4d1e8f0a3b6c9d2e5f8a1",
  "wave": 2,
  "generation": {
    "closureHash": "sha256-ddeeff...",
    "bootId": "new-boot-uuid-..."
  },
  "probeResults": [
    { "control": "anssi-bp028-ssh-no-password", "status": "passed", "evidence": "..." }
  ]
}
```

`rollout` is the same content hash the agent received in `target.rollout` and persisted to its cache; the CP looks up `(hostname, rollout)` in `host_rollout_state` and asserts the agent is confirming a rollout it actually dispatched.

**Response:** `204 No Content` on acceptance, `410 Gone` if the rollout was cancelled or the wave already failed (agent then triggers local rollback on its own).

### 4.3 `POST /agent/report`

Out-of-band state reports: activation failure, probe failure, voluntary rollback. Distinct from `/checkin` so that failure reports don't interleave with normal polling cadence.

```json
{
  "hostname": "m70q-attic",
  "event": "activation-failed",
  "rollout": "stable@def456",
  "details": {
    "phase": "switch-to-configuration",
    "exitCode": 1,
    "stderrTail": "..."
  }
}
```

### 4.4 `GET /agent/closure/<hash>`

Optional. If the host cannot reach the binary cache directly (restricted network), the control plane can proxy closures. Preference remains: agents fetch from cache, not control plane - this endpoint exists as a fallback, not a default path.

### 4.5 Enrollment endpoints (nixfleet #9)

Out of scope for this RFC in detail. Summary:

- `POST /enroll` - accepts bootstrap token + CSR, returns signed cert. Token is burned on use.
- `POST /agent/renew` - accepts current cert (mTLS) + CSR, returns refreshed cert.
- `POST /agent/bootstrap-report` - pre-cert reporting path for failures that prevent normal cert provisioning.

#### Bootstrap report

Agents that fail enrollment cannot use the mTLS-gated `POST /agent/report` to surface the failure (no cert yet). `POST /agent/bootstrap-report` exists for this case alone.

**Authentication.** Bound to a hostname + agent-supplied pubkey via the same bootstrap token used by `POST /enroll`. The token is NOT consumed - multiple bootstrap reports may fire while the operator iterates on the underlying issue. The token's lifetime gates the window.

**Allowlisted events.** Only `TrustError` and `EnrollmentFailed` events are accepted on this endpoint. Anything else is `400`. The allowlist enforces the path's narrow purpose: surfacing why enrollment is broken, not generic agent telemetry.

**No nonce consumption.** Standard `/agent/report` consumes a per-checkin nonce to bind the report to a specific server view. Bootstrap reports happen before the agent has a checkin nonce; nonce binding is not enforced. The token + hostname + pubkey + event allowlist together constrain the abuse surface.

**Response.** `204 No Content` on accept; the CP records the event in `host_reports` (same ring as post-enrollment events) so the operator dashboard sees pre-cert failures in the same panel as post-cert ones. Subsequent successful `/enroll` does not retroactively rewrite the bootstrap-report rows.

### 4.6 `GET /v1/rollouts/<rolloutId>`

Distributes the signed `RolloutManifest` (RFC-0002 §4.4) to agents. mTLS-gated like every other endpoint. The CP serves the on-disk pre-signed pair byte-for-byte; it does not re-derive, re-sign, or otherwise transform the manifest.

**Path parameter.** `rolloutId` is the content hash exactly as the CP advertised it in `/agent/checkin` responses. Hex, lowercase, full SHA-256 (64 chars).

**Response.** Two body shapes, served via the standard HTTP `Accept` content-negotiation pattern:

- `Accept: application/json` (default) returns the manifest JSON bytes.
- `Accept: application/octet-stream` returns the raw signature bytes (`<rolloutId>.sig`).

Agents fetch both. Implementations MAY also expose a single endpoint that returns both bundled (e.g. `application/json` with the signature in a sibling `X-Nixfleet-Signature` header); the wire-test harness asserts both shapes round-trip identically.

**Status codes.**

- `200 OK` - manifest found, body served.
- `404 Not Found` - `rolloutId` is unknown to the CP (never adopted, or evicted post-rollout-completion).
- `503 Service Unavailable` - CP recently rebuilt and has not yet reloaded the rollouts directory; agent retries after `nextCheckinSecs`.

**Idempotency + caching.** Manifests are immutable by content-address: a given `rolloutId` always returns the same bytes, or `404` if it never existed. Agents that have already cached a manifest do NOT need to re-fetch on every checkin - string equality against the cached `rolloutId` is sufficient. Defensive re-fetches (e.g. on agent restart) are safe but wasteful.

**No write side.** There is no `POST` or `PUT` on this endpoint. Manifests are produced by CI alone; the CP holds no signing key for rollouts. Operator workflows that need to "edit a rollout plan" require a new commit (which produces a new `rolloutId`).

## 5. Polling cadence

- **Default interval.** 60s, controlled server-side via `nextCheckinSecs` in the checkin response.
- **Backoff on error.** Exponential with jitter, capped at the channel's `reconcileIntervalMinutes`. Network errors do not drain the confirm window - `/confirm` retries aggressively (up to 5×) within the window to survive transient failures.
- **Load shaping.** Control plane can vary `nextCheckinSecs` per-host to smooth thundering herds after a push (e.g. assigning each host a slot within the polling window based on a hash of its hostname).
- **Idle hosts.** A host with no pending target polls at the channel's idle cadence (can be much longer - weekly for `edge-slow`).

## 6. Versioning

- **Protocol major version** in header. v1 → v2 is a breaking change; running mixed versions is disallowed and fails at check-in with a clear message. Upgrade path: control plane supports N and N+1 simultaneously; operators upgrade agents, then retire control plane's N support.
- **Schema evolution within a major.** Fields may be added; agents and control plane MUST ignore unknown fields. Required fields never change meaning. Removing a field requires a major bump.
- **Agent version (informational).** Control plane refuses agents older than its declared minimum, emits events for newer agents (may indicate staged upgrade in progress).

## 7. Security model

**Defended against:**

- **Passive network observer.** TLS 1.3 - sees only traffic shape.
- **Active on-path attacker without a cert.** mTLS fails the handshake; no data exposed.
- **Compromised non-target agent.** Cert only authorizes its own hostname; cannot request targets for other hosts, cannot submit reports for other hosts. Control plane enforces `cert.CN == request.hostname` on every endpoint.
- **Compromised control plane - closure forgery.** Cannot learn secrets (zero-knowledge, nixfleet #6). Can serve a different closure hash as target → agent fetches from attic, verifies attic's ed25519 signature against the pinned attic public key (ARCHITECTURE.md §4), refuses unsigned or foreign-signed closures.
- **Compromised control plane - stale-closure replay.** A compromised CP cannot forge closures but could point hosts at an older-but-still-validly-signed closure to block security fixes. Mitigation: every check-in response references a CI-signed `fleet.resolved` revision; the agent fetches that artifact (directly from cache or via the CP) and refuses any target whose backing `fleet.resolved.meta.signedAt` is older than `channel.freshnessWindow` (per-channel declaration in minutes, required, no default - RFC-0001 §2.3). The freshness window is itself inside the signed artifact, so a compromised CP cannot widen it.
- **Replay.** Confirm requests include `bootId`; the control plane rejects a confirm whose `bootId` doesn't match the expected new boot.

**Not defended against (explicit):**

- **Compromised host (root).** If the host's TLS key is stolen, the attacker can act as that host until the cert is revoked. Mitigated by short cert lifetime + TPM-backed keys (future issue).
- **Denial of service.** Out of scope for this RFC. Rate limiting, fail2ban-style protections, and similar are operational concerns.
- **Malicious control-plane operator.** Is explicitly a trusted role (can push any generation to any host). The security boundary is between the fleet and outsiders, not between operators and hosts.

## 8. Offline behavior

- **Agent caches the last check-in response** on disk. If the control plane is unreachable, the agent continues to operate at its current generation. It does not auto-revert, does not auto-upgrade.
- **Prolonged offline window.** If check-in fails for longer than `channel.offlineGraceSecs` (default: 7 days), the agent emits a local systemd journal warning but takes no action. Action is an operator decision.
- **Clock skew tolerance.** All deadlines (confirm window, cert validity) carry ≥ 60s slack to absorb typical host↔CP clock drift.

## 9. Open questions

1. **Per-host pinning for debugging.** Should operators be able to pin a host to a specific generation outside normal rollouts ("don't touch this, I'm debugging")? Leaning yes, via a `freeze` flag in fleet.nix or a control-plane-side override - but this is declarative-intent-breaking, so needs careful design.
2. **Streaming vs polling.** SSE or long-polling for the checkin endpoint would reduce latency for event-driven rollouts (no need to wait for next poll). Deferred to v2; pure polling is simpler to reason about and adequate for nixfleet's target fleet sizes.
3. **Multi-control-plane.** Agents talking to a quorum of CPs for HA. Out of scope for v1; single control plane with standard HA (pacemaker, k8s statefulset) is the expected deployment.

### Resolved in v0.2

- **Closure signing** (was: should CP sign `target` responses?). Resolved: closures are signed by attic (not the control plane), `fleet.resolved` is signed by CI, both verified by the agent. CP `target` responses are not independently signed - they carry references (closure hash, `fleet.resolved` revision) that the agent verifies against their respective signing roots. See ARCHITECTURE.md §4 and §7 "stale-closure replay" above.

---

## Appendix: Relationship between the three RFCs

```
  RFC-0001 (fleet.nix)          "what do I want?"
       │
       │  produces fleet.resolved
       ▼
  RFC-0002 (reconciler)          "what should happen next?"
       │
       │  emits per-host intents
       ▼
  RFC-0003 (agent protocol)      "how do intents reach hosts and
                                  how does observed state come back?"
       │
       │  updates observed state
       ▼
  RFC-0002 (reconciler, next tick)
```

The loop is:

1. RFC-0001 defines desired state.
2. RFC-0002 compares desired to observed and emits intent.
3. RFC-0003 ships intent to agents and returns observations.
4. Loop forever. Every tick is idempotent. Every decision has a written reason.

That's the whole system. Everything else in nixfleet - CLI, compliance, scopes, darwin support - is an accessory to this loop.

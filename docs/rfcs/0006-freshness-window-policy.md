# RFC-0006: Freshness window policy

**Status.** Draft.
**Targets.** v0.3.
**Depends on.** RFC-0001 (channel schema), RFC-0003 (agent protocol), ARCHITECTURE.md §5.
**Scope.** Make replay protection explicit, machine-checkable, and recoverable. Adds: explicit freshness fields on the agent-target wire, time-source policy per channel, operator visibility for stalled channels and long windows, `TimeSourceUnavailable` event class.

## 1. Motivation

ARCHITECTURE.md §5 names the threat:

> *Control plane host is compromised. Attacker can: refuse to serve updates (DoS), serve stale-but-valid targets (replay). Mitigation: agents refuse to accept targets older than a configurable freshness window signed by CI.*

v0.2 implements most of this. The CP enforces `freshnessWindowMinutes` on `meta.signedAt` at fetch time. The agent has `freshness.rs` and the `fleet-harness-stale-target` scenario verifies it. `mkFleet` requires `freshnessWindow` per channel and enforces the cross-field invariant `freshnessWindow ≥ 2 × signingIntervalMinutes`.

What is missing:

1. **Explicit freshness on the wire.** The agent currently derives the window from its local `freshness.rs` configuration. A channel that needs a tighter window for some hosts has no clean mechanism. The window should ride the signed target.
2. **Time-source policy.** v0.2 trusts the host's local clock implicitly. A maliciously-skewed clock (or just an unsynchronized one) silently breaks the protection.
3. **Operator visibility.** A channel approaching its window expiry should warn before agents start refusing. A long window is a configuration smell that should be visible in fleet status.
4. **Hard floor.** Nothing in v0.2 prevents `freshnessWindow = "30m"` on a channel whose `signingIntervalMinutes = 60` — the existing invariant catches this case but a `freshnessWindow = "5m"` on a channel that signs once an hour passes the invariant and produces near-useless replay protection.

This RFC fills those four gaps. Most of v0.2's freshness machinery is reused; the additions are surface area, not core mechanism.

## 2. Schema additions

### 2.1 Channel-level

```nix
channels.production = {
  rolloutPolicy            = "canary-conservative";
  signingIntervalMinutes   = 60;
  freshnessWindow          = 1440;       # already required, unchanged
  freshnessHardFloorMinutes = 60;        # NEW — see §2.3, default 60
  timeSource = {                          # NEW — see §4
    ntp = [ "time.cloudflare.com" "time.nist.gov" ];
    maxSkewSeconds = 300;
  };
};

channels.gov-prod = {
  # ...
  freshnessWindow = 1440;
  timeSource = {
    signedTime = {
      provider = "roughtime";
      url = "https://time.gov.example/roughtime";
      pubkey = "...";
    };
    fallback.ntp = [ "internal-ntp.example" ];
    maxSkewSeconds = 60;
  };
};
```

### 2.2 Defaults

| Field | Online channels | Air-gap channels (RFC-0007) |
|---|---|---|
| `freshnessWindow` | required, suggested 24h | required, suggested 90d |
| `freshnessHardFloorMinutes` | 60 (1h) | 60 (1h) — same; air-gap windows are about the upper bound |
| `timeSource.maxSkewSeconds` | 300 (5min) | 60 (1min) — air-gap typically uses signed-time, can be tighter |
| `timeSource` | NTP defaults to `["time.cloudflare.com" "time.nist.gov"]` | no NTP default — operator declares signed-time or internal NTP explicitly |

### 2.3 Hard floor

`freshnessWindow < freshnessHardFloorMinutes` is rejected at `mkFleet` evaluation time with a clear error. The floor is per-channel-overridable (rare — for example a channel with `signingIntervalMinutes = 5` may want `freshnessHardFloorMinutes = 15`).

There is no hard ceiling. Long windows are sometimes correct (frozen audit channels, compliance-locked baselines). The framework adds friction (§5) instead of forbidding them.

## 3. Wire-protocol additions

### 3.1 Agent target shape

RFC-0003 §4.1 `CheckinResponse.target.activate` gains explicit freshness fields:

```rust
pub struct ActivateBlock {
    // ... existing fields ...
    pub fleet_resolved_signed_at: DateTime<Utc>,    // NEW
    pub freshness_window_seconds: u64,              // NEW
    pub freshness_hard_floor_seconds: u64,          // NEW
    pub time_source: TimeSourcePolicy,              // NEW (per-channel snapshot)
}
```

These fields are projections from `meta.signedAt` and the channel's `freshnessWindow` / `freshnessHardFloorMinutes` / `timeSource`. The CP copies them from the signed `fleet.resolved.json` into the per-host target response. The CI signature on `fleet.resolved.json` covers them; the CP cannot widen the window or weaken the time-source policy.

Pre-RFC-0006 agents ignore the new fields (existing `serde(default)` convention). Post-RFC-0006 agents enforce them in addition to whatever local config they may carry — local config is used only as a fallback when target fields are absent (i.e., when serving from a pre-RFC-0006 CP).

### 3.2 Agent verification

On every checkin response with a target, the agent verifies, in order:

1. Existing v0.2 verifications: rollout-manifest signature, content-address, host membership (RFC-0003 §4.1).
2. **Time-source freshness:** establish local time within `time_source.maxSkewSeconds` of the configured time source (§4). On failure: emit `TimeSourceUnavailable`, refuse to evaluate freshness, hold the current generation, do not converge to the new target.
3. **Freshness:** `now() - fleet_resolved_signed_at <= freshness_window_seconds`. On failure: emit `StaleTargetRejected`, refuse to converge, hold the current generation.
4. Existing v0.2 activation flow if 1–3 pass.

The agent does not stop working on freshness or time-source failures. It stays on the current generation, continues running existing services, and continues to phone home. Only convergence to *new* targets is blocked. Freshness failure is a control-plane-trust signal, not a host-health problem.

### 3.3 Event additions (RFC-0003 §4.3)

Two new `ReportEvent` variants, additive:

- `StaleTargetRejected { observed_age_seconds, signing_timestamp, freshness_window_seconds }`
- `TimeSourceUnavailable { configured_sources, last_attempt_at, last_error }`

Both are unsigned (operator-surface, no fleet gate reads them — matching the existing `ActivationDeferred` / `ClosureQuarantined` precedent per the v0.2 changelog).

## 4. Time-source policy

The agent does not trust the CP for time (pull-only model, RFC-0003 §1). It validates its local clock against an independent source declared per channel.

### 4.1 NTP source

```nix
timeSource = {
  ntp = [ "time.cloudflare.com" "time.nist.gov" ];
  maxSkewSeconds = 300;
};
```

Agent behavior: verify that the host's `chronyd` / `systemd-timesyncd` reports synchronized within `maxSkewSeconds` of one of the declared sources. If the host has its own timesync daemon configured (likely — most NixOS hosts do), the agent reads the daemon's reported skew rather than running its own NTP query.

### 4.2 Signed-time source

For high-trust environments and air-gap (RFC-0007), a signed-time service is preferable:

```nix
timeSource = {
  signedTime = {
    provider = "roughtime";       # or "tlsdate" — pluggable
    url = "https://time.gov.example/roughtime";
    pubkey = "...";
  };
  fallback.ntp = [ "internal-ntp.example" ];   # optional
  maxSkewSeconds = 60;
};
```

Roughtime is the recommended protocol (open spec, deployable). v0.3 ships a generic signed-time fetcher with Roughtime support and a documented adapter pattern for other providers.

### 4.3 Failure mode

If the agent cannot establish a time source within `maxSkewSeconds`:

- **Refuses to evaluate freshness** — neither accepts nor rejects targets.
- **Logs `TimeSourceUnavailable` event** with last-attempt details.
- **Continues running the current generation** — services do not stop.

This is preferred to silent acceptance: an agent with an unverifiable clock is in an undefined state, and undefined-state agents do not move forward. Operators see this in fleet status (§5) as "unable to assess freshness."

## 5. Operator visibility

The CP's `/v1/hosts` and `/v1/channels/<name>` endpoints, and the `nixfleet status` CLI, surface:

- **Current target's age.** `now - fleet_resolved_signed_at`.
- **Distance from window expiry.** e.g., "expires in 3h17m".
- **Hosts that have rejected the current target as stale.** Count and list. Sourced from the `host_reports` ring (`StaleTargetRejected` events).
- **Hosts with `TimeSourceUnavailable`.** Count and list.

A channel that has stalled (no new commits for > 75% of `freshnessWindow`) emits a `StaleChannelWarning` to operators **before** the window expires. This is preventive: operators see the warning and either commit a no-op refresh or extend the window with rationale, before agents start refusing.

Channels with `freshnessWindow > 7d` (online) or `> 90d` (air-gap) are flagged in fleet status as `long-freshness-window — confirm rationale`. This is friction by design: long windows are sometimes correct but are also the most common configuration mistake in this area.

## 6. Edge cases

- **Skewed local clock.** `maxSkewSeconds` enforcement catches it. Operator runs NTP/chrony correction; until corrected, the host stays on its current generation.
- **Channel intentionally stalled (frozen for audit).** Operator extends the window: `channels.audit-frozen.freshnessWindow = 60d` with a rationale comment. The framework does not assume frozen channels are unintentional.
- **Air-gap import older than the channel's freshness window.** RFC-0007 §6 covers air-gap freshness; the bundle's signing time is what matters.
- **Wave promotion when freshness will expire mid-rollout.** The reconciler warns at rollout-start time if `target.age + estimated_rollout_duration > freshness_window`. The operator either restarts the rollout against a fresh target or extends the window.
- **Agent online but unable to reach NTP (firewalled environment).** Falls into `TimeSourceUnavailable`. Operator either provides an internal NTP source or moves the channel to `signedTime`.

## 7. Trust analysis

**What this RFC adds.** A concrete, machine-checkable replay-protection contract. A compromised CP serving a stale-but-valid target now fails closed within `freshnessWindow` of the original signing time — without operator action, regardless of agent-local configuration drift.

**What it does not add.** Protection against an attacker who can also tamper with the agent's time source. Mitigation is the channel-declarable time-source policy: high-trust channels use signed time, low-trust channels use public NTP. The operator has explicit visibility into which choice each channel made (it's in the channel definition, git-tracked, in the protected-branch review path).

**Failure mode worth stating.** A misconfigured `freshnessWindow = "1y"` on a production channel turns this protection off in practice. The hard floor (default 1h) prevents the obvious mistake; long-window flagging in fleet status is the protection against the less-obvious one. Channel definitions are git-tracked; `freshnessWindow` should be in the protected-branch review path.

## 8. Build phases

This RFC is small enough to land in one phase. Continues `ARCHITECTURE.md` §6 numbering after RFC-0005.

- **Phase 20 — Freshness hardening.** Five sub-deliverables, parallelizable:
  - 20.1 Schema additions (`freshnessHardFloorMinutes`, `timeSource`); mkFleet enforcement.
  - 20.2 Wire-shape additions (`AgentTarget` freshness fields); CP populates from signed source.
  - 20.3 Agent enforcement of `freshness_window_seconds` from the target (in addition to local config).
  - 20.4 Time-source policy: NTP synchronization check via the host's existing timesync daemon; `TimeSourceUnavailable` event class.
  - 20.5 Operator-visibility additions: `StaleChannelWarning`, long-window flagging, `nixfleet status` columns.

Roughtime / signed-time-source support is a separate sub-deliverable (20.6, optional for v0.3) — substantial dependency, useful but not blocking the rest. Lean: ship NTP-based time-source in v0.3, Roughtime as v0.3.x or v0.4 follow-up.

## 9. Falsifiable done criteria

1. A CP that serves an artificially-aged `fleet.resolved.json` (timestamp moved back beyond `freshness_window_seconds`) is detected by every agent that polls it; agents emit `StaleTargetRejected`; no host converges.
2. An agent whose host has a maliciously-skewed local clock (forward by > `maxSkewSeconds`) refuses to evaluate freshness and emits `TimeSourceUnavailable`.
3. A channel that has stalled past 75% of its window emits a `StaleChannelWarning` visible in fleet status before any agent has rejected.
4. The hard floor cannot be bypassed: a `freshnessWindow = "30m"` declaration on a channel with the default `freshnessHardFloorMinutes = 60` is rejected at evaluation time with a clear error.
5. A channel with `freshnessWindow > 7d` (online) is flagged in fleet status with the long-window indicator.

## 10. Open questions

- **Default `maxSkewSeconds`.** 5 minutes balances NTP accuracy against replay window. Open to tightening (1 minute) for high-trust channels — already the suggested air-gap default.
- **Time-source-daemon coupling.** Reading skew from `chronyd` vs `systemd-timesyncd` requires backend-specific code. v0.3 ships systemd-timesyncd support (most NixOS hosts) + a documented extension pattern. Chrony adapter as needed.
- **Roughtime adoption.** Open spec but limited deployment. v0.3 ships the integration; whether to recommend it depends on whether customer environments have a Roughtime server reachable (most don't yet).
- **Stale-channel warning threshold.** 75% is a guess. Worth surveying after a quarter of operation against real channels.

## 11. One-sentence summary

**Every signed target carries an expiry the agent independently verifies against a declared time source; a CP that lies about freshness — or a host whose clock is wrong — is detected within a poll cycle, no operator action required.**

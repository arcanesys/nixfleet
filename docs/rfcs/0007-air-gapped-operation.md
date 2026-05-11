# RFC-0007: Air-gapped operation

**Status.** Draft.
**Targets.** v0.3.
**Depends on.** ARCHITECTURE.md (especially §5 control-plane failure case), RFC-0001 (channel schema), RFC-0003 (agent protocol), RFC-0006 (freshness in air-gap).
**Scope.** First-class deployment mode for environments with no internet egress: energy operators, water utilities, defense-adjacent contractors, healthcare critical systems. The trust model already supports this — every artifact is self-verifying. This RFC makes the workflow explicit, the sovereign-cache transport role explicit, and ships the bundle tooling.

## 1. Motivation

The v0.2 trust model is air-gap-ready by accident: closures are content-addressed and signed by attic, `fleet.resolved.json` is signed by CI, agents verify everything against pinned trust roots, the CP holds no secrets. None of these properties depend on internet reach.

What is missing is the workflow. An operator running a regulated air-gap site needs to know:

- How releases enter the air-gap (transport, format, verification).
- What the sovereign cache's role is (re-signing? transport-only?). This matters: re-signing means a new trust root inside the air-gap, transport-only means the existing trust roots cover everything.
- How freshness applies when bundles are days or weeks old by design (RFC-0006 cross-reference).
- How key rotation crosses the air-gap (RFC-0005 §7 cross-reference).

The mechanism is small. The contract is the bulk of the work.

## 2. Non-goals

- **Two-way air-gap.** Telemetry, support bundles, or any reverse channel from the air-gap is the customer's responsibility. RFC-0007 covers the inbound path only.
- **Real-time sync.** By definition not possible. Channels in air-gap mode update at human cadence.
- **Auto-discovery of new releases from inside the air-gap.** The operator pulls a bundle, validates it, imports it. There is no automatic mechanism that bridges the gap.

## 3. Model

Three environments connected by signed bundles:

```
   online build env             air-gap entry point         air-gapped fleet
   ─────────────────             ──────────────────          ─────────────────
   Forgejo + CI                  signed-bundle inbox         sovereign attic
   attic + signing keys ───────▶ verification host    ────▶  control plane
   fleet.resolved + closures     bundle import tool          agents
                                 (validates, signs receipt)
```

- The build environment is unchanged from v0.2.
- The air-gap entry point is a documented station (typically a kiosk machine with a known boot image) that accepts bundles from approved media (USB, one-way data diode, signed optical media), verifies them against the configured trust roots, and pushes them into the sovereign cache.
- The sovereign cache is just `attic`, run inside the air-gapped environment.

### 3.1 Sovereign attic is transport-only

This is the load-bearing decision. The sovereign attic does **not** re-sign closures. Closures imported from a bundle keep their original attic-key signatures (the same key that signed them in the online build environment, declared in `nixfleet.trust.cacheKeys`). The sovereign attic forwards bytes; it does not re-attest to them.

Consequence: agents inside the air-gap trust the same `cacheKeys` they would trust online. There is no "sovereign cache key" trust root to manage. A compromised sovereign attic cannot inject malicious closures because it cannot produce signatures under a key the agents trust.

Operationally this means: the sovereign attic's own internal signing key (attic generates one per instance) is unused by the framework — agents never check it. Setting `attic` up in a "no signing required" mode is the recommended deployment.

If a customer wants their sovereign cache to also re-sign for defense-in-depth (e.g., to prove "this closure passed the air-gap import check"), that can be layered on top — the agent supports multiple `cacheKeys` simultaneously per the existing v0.2 contract. Out of scope for the framework's recommended deployment.

## 4. Bundle format

A bundle is a signed tarball containing:

```
bundle-2026-05-14.tar
├── manifest.json              # bundle metadata, signed by CI release key
├── manifest.json.sig
├── fleet/
│   ├── fleet.resolved.json    # signed per RFC-0001
│   ├── fleet.resolved.json.sig
│   ├── revocations.json       # signed per ARCHITECTURE.md §6 Phase 10
│   └── revocations.json.sig
├── rollouts/
│   ├── <rolloutId>.json       # signed per RFC-0002 §4.4
│   └── <rolloutId>.json.sig
├── closures/
│   └── <hash>.nar.xz          # closure tarballs (already attic-signed inline; no separate .sig file)
└── import-instructions.md     # operator-readable procedure (humans, not machines)
```

The manifest declares: which channels this bundle updates, the previous channel-pointer it expects (so out-of-order imports are detected), the CI commit range, and the bundle's expiry. Bundles older than the channel's air-gap freshness window (RFC-0006) are rejected at import.

The bundle has its own signature on the manifest, in addition to the per-artifact signatures. Reasoning: early failure detection at verify time, before any artifact is opened. A tampered bundle fails the manifest signature check immediately rather than being detected piecewise as artifacts are extracted.

Closure signatures live inline in the `.nar.xz` archive (per attic's existing format); no separate `.sig` file in the bundle for closures. Per-closure verification happens at sovereign-attic import time AND at agent-fetch time, against the same `cacheKeys` trust root.

## 5. Tooling

A new crate, `nixfleet-bundle`, matches the existing `nixfleet-release` / `nixfleet-verify-artifact` pattern (single-purpose binary, no daemon, no state):

```sh
# online build environment
nixfleet-bundle export \
  --channel stable \
  --since <previous-bundle-ref> \
  --output ./bundle-2026-05-14.tar

# air-gap entry point (offline)
nixfleet-bundle verify ./bundle-2026-05-14.tar
nixfleet-bundle import ./bundle-2026-05-14.tar \
  --sovereign-cache https://attic.internal.example
```

`bundle verify` is a separate command from `import` deliberately: in higher-security environments the verification host and the import host are different machines with different access policies. A combined non-interactive form (`nixfleet-bundle apply`) is provided for one-way diode setups where verify-then-import in two steps is impractical.

The framework also exposes flake apps that wrap the binary for the common cases: `nix run .#bundle-export -- --channel stable`, `nix run .#bundle-verify -- ./bundle.tar`, etc. The binary is the lower-level interface; the flake apps are operator ergonomics.

## 6. Freshness in air-gap

Channels in air-gap mode declare an explicit longer freshness window per RFC-0006, plus an air-gap-specific staleness for the bundle itself:

```nix
channels.airgap-prod = {
  airgap.enabled       = true;
  airgap.maxStaleness  = "30d";     # bundle import freshness
  freshnessWindow      = 129600;     # 90d in minutes — CI-signing-time freshness
  timeSource = {
    signedTime = { provider = "roughtime"; url = "..."; pubkey = "..."; };
    fallback.ntp = [ "internal-ntp.example" ];
    maxSkewSeconds = 60;
  };
};
```

Two timestamps matter:

- **Bundle signing time** — when CI produced the artifacts. Compared against the channel's `freshnessWindow` per RFC-0006; this is the agent's replay-protection contract.
- **Bundle import time** — when the sovereign cache received the bundle. Compared against `airgap.maxStaleness`; this is the operator's "are we current?" contract.

The agent uses signing time, not import time, for freshness verification. Import time is operator metadata recorded in the import receipt (§7) and surfaced in fleet status; it does not gate convergence.

Agents that have been offline since before the most recent import use the import time as a recovery anchor (i.e., for computing "how long have we been operating on a stale view of the channel"); operators see this as a per-host staleness indicator distinct from the channel-level freshness window.

For time source: air-gap channels MUST NOT use the public NTP defaults from RFC-0006 §4 (Cloudflare/NIST aren't reachable). The framework refuses to evaluate an air-gap channel without an explicit `timeSource` declaration. Recommended: a signed-time service (Roughtime or equivalent) with an internal NTP fallback. Internal NTP-only is acceptable for less stringent environments.

## 7. Control plane in air-gap

The CP runs inside the air-gap with no special configuration. It polls the sovereign cache (or receives a webhook from `nixfleet-bundle import`) for new channel pointers. Its signature verification continues as in v0.2: it verifies CI signatures on `fleet.resolved.json`, regardless of whether the bundle came over the internet or via USB.

The CP in air-gap holds the same trust roots as the online version. Trust origins (org root, CI release key, attic cache key) are deployed into the air-gap at the same enrollment time as the rest of the infrastructure. RFC-0005 §7 rotation procedures apply with one additional step: "rotation envelope traverses the air-gap as a bundle."

The import receipt is a small signed JSON written by `nixfleet-bundle import` to a known location:

```json
{
  "bundleSha256": "...",
  "importedAt": "2026-05-14T10:23:00Z",
  "operator": "alice",
  "verifiedSignatures": [ "ciReleaseKey:...", "atticKey:..." ]
}
```

Receipt is signed by the import operator's key (an SSH key registered for this purpose; not part of the framework's trust chain — purely operator-facing accountability). Surfaced in fleet status alongside channel staleness.

## 8. Operator procedure (compact form)

```
1. online: nixfleet-bundle export --channel <c> --since <prev> --output bundle.tar
2. transfer bundle to air-gap entry point via approved media
3. air-gap entry point: nixfleet-bundle verify bundle.tar
     - verifies manifest signature against trusted CI key
     - verifies fleet.resolved.json + revocations.json + each rollout manifest signature
     - verifies bundle expiry vs current air-gap clock
     - verifies channel pointer expectations (previous-pointer matches)
4. air-gap entry point: nixfleet-bundle import bundle.tar
     - re-verifies (idempotent; survives operator running verify on a different host)
     - pushes closures into sovereign attic (no re-signing — pass-through)
     - publishes fleet/revocations/rollout artifacts to a path the CP polls
     - records signed import receipt
5. CP on next poll picks up the new channel pointer, reconciles normally
6. agents on next poll fetch the new target, fetch closures from sovereign attic, activate
```

The full chain, online commit to first agent activation, is human-paced (typically minutes to hours depending on operator process) but is end-to-end signature-verified at every step.

## 9. Failure cases

- *Bundle signature invalid.* Rejected at verify; never enters the sovereign cache.
- *Bundle expired.* Rejected at verify; operators must re-export.
- *Out-of-order bundle (skips an expected previous channel pointer).* Rejected unless `--allow-skip` is passed with a rationale; logged.
- *Sovereign cache compromised.* Closures still verify against pinned `cacheKeys` on agents; an attacker who replaces a closure cannot make agents accept it. DoS is possible (delete or block fetch); fleet stalls until the cache is restored from re-imported bundles.
- *Operator imports a bundle to the wrong channel.* Channel-pointer signatures bind to channel name; mismatched bundle is rejected at verify.
- *Bundle imported but never reaches agents (network partition inside air-gap).* Agents cache last known target and continue running; the new target activates when the partition heals.
- *Time-source unavailable inside air-gap.* Per RFC-0006 §4.3: agents refuse to evaluate freshness, hold current generation, emit `TimeSourceUnavailable`. Operator either restores the signed-time service or extends the channel's `freshnessWindow` with rationale.
- *Import operator's signing key compromised.* Import receipts under that key become untrustworthy; subsequent imports use a new key. The receipts are accountability metadata, not part of the agent-verification chain — no agent action required.

## 10. Trust analysis

**Properties retained from v0.2.**

- Every artifact is self-verifying against pinned trust roots.
- The CP holds no secrets and forges no trust.
- A compromised sovereign cache cannot inject malicious closures.

**Properties added.**

- Documented bundle format with a manifest signature for early-failure detection.
- Explicit operator workflow with a verified import receipt.
- Explicit air-gap freshness contract that does not weaken the online freshness contract.

**What this RFC does not protect against.**

- A compromised CI release key signing a malicious bundle. RFC-0005 (threshold-signed channels) is the answer for high-stakes air-gap deployments.
- An operator importing a malicious bundle whose signatures verify because the attacker has the keys. Same as above.
- A leaked import receipt key being used to fake an import. Fix: rotate the receipt key.

## 11. Build phases

- **Phase 21 — `nixfleet-bundle` crate + bundle format + verify/import + air-gap channel schema.** Single phase, all sub-deliverables tightly coupled:
  - 21.1 Crate scaffold; bundle manifest types in `nixfleet-proto`.
  - 21.2 `bundle export` (online side).
  - 21.3 `bundle verify` (offline, no network).
  - 21.4 `bundle import` (offline, writes to sovereign attic + CP-polled paths).
  - 21.5 Air-gap channel schema (`airgap.enabled`, `airgap.maxStaleness`); mkFleet enforcement of explicit `timeSource` for air-gap channels.
  - 21.6 microvm.nix scenario simulating the full pipeline (online build → bundle → offline verify → import → agent activation).

## 12. Falsifiable done criteria

1. A complete air-gap workflow can be demonstrated end-to-end: online commit → bundle export → physical transfer (simulated as `cp` in the microvm scenario) → verify → import → agent activation, with every step independently signature-verifiable.
2. A bundle with one bit flipped in any signed component is rejected at verify.
3. A CP operating in air-gap can complete a full reconcile cycle with no DNS, no NTP egress, and no internet-bound traffic of any kind.
4. The sovereign cache can be lost and rebuilt from re-imported bundles without fleet impact beyond fetch latency.
5. An auditor inside the air-gap can produce the full provenance chain for any host's current closure: which bundle imported it, when, who approved the import, what CI commit produced it.
6. An air-gap channel declared without an explicit `timeSource` is rejected at evaluation time with a clear error.

## 13. Open questions

- **Telemetry from inside the air-gap.** Some customers want a one-way channel for "fleet healthy" beacons exfiltrated for upstream support. Out of scope here; deserves its own spec. Likely solution: a signed daily summary written to a documented path, picked up by the customer's existing one-way egress process.
- **Diode-friendly tooling.** Some environments use one-way data diodes that prohibit bidirectional handshakes. The combined `bundle apply` command should be testable without any return path; verify this with a customer who actually uses diodes before declaring done.
- **Bundle compression and partial transfer.** For very large fleets, full closure transfer over USB media may be impractical. Worth specifying a partial-bundle format (delta against previous) before the first large-fleet pilot. Defer to v0.4 unless a customer asks.
- **Threshold signing across the air-gap.** RFC-0005 §6 signing sessions assume a forge-reachable transport. Air-gap threshold signing needs a session-bundle round trip. RFC-0005 §12 lists this as an open question; resolution is a v0.4 cycle.

## 14. One-sentence summary

**The air-gap is a USB cable's worth of latency between commit and convergence — every artifact still self-verifies against the same trust roots, the sovereign cache forwards bytes without re-signing, and the workflow is documented as a first-class deployment mode rather than a clever derivation.**

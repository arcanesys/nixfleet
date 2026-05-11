# Boundary contracts

The single authoritative reference for every artifact, key, and format that crosses a layer boundary during v0.2. If it is not listed here, it is not a contract - it is implementation detail that can change without coordination.

Every entry declares:
- **Producer** - the layer/component that emits the artifact.
- **Consumer(s)** - layers/components that read it.
- **Schema/version** - current version and the discipline for evolving it.
- **Verification** - what a consumer must check before trusting the content.

Boundaries cross between three layers:
- **CI / infra** - M70q coordinator, out of tree; lives in `fleet` repo.
- **Nix declarative** - this repo's `lib/`, `modules/`, + `nixfleet-compliance`.
- **Rust runtime** - this repo's `crates/` (agent + control plane).

---

## I. Data contracts

### 1. `fleet.resolved.json`

| | |
|---|---|
| **Producer** | CI (lab CI invokes the Nix layer's eval) |
| **Consumer** | Control plane, agents (fallback direct fetch) |
| **Schema** | v1 - shape defined in RFC-0001 §4.1 |
| **Canonicalization** | JCS (RFC 8785), see §IV |
| **Signature** | CI release key (see §II #1) |
| **Metadata** | `meta.signedAt` (RFC 3339), `meta.ciCommit`, `meta.schemaVersion`, `meta.signatureAlgorithm` (`"ed25519"` \| `"ecdsa-p256"`; optional, defaults to `"ed25519"`) |

**Evolution discipline.** Within v1, fields may be added; consumers MUST ignore unknown fields. Removing or changing the meaning of a field requires `schemaVersion: 2` and a migration window. `meta.signatureAlgorithm` was added after the initial `schemaVersion: 1` draft - artifacts without the field MUST be interpreted as `"ed25519"` for backward compatibility.

**Consumer MUST verify before use:**
1. JCS bytes match the canonicalized payload.
2. `meta.signatureAlgorithm` (default `"ed25519"`) matches the algorithm of the pinned `nixfleet.trust.ciReleaseKey`.
3. Signature verifies against the pinned `nixfleet.trust.ciReleaseKey` using the declared algorithm.
4. `(now − meta.signedAt) ≤ channel.freshnessWindow` (units: minutes; see RFC-0001 §4.1).
5. `meta.schemaVersion` is within the consumer's accepted range.

**Producer pipeline (`nixfleet-release`).** The framework ships one orchestrator binary that produces this artifact: eval `fleet.resolved` → filter expired pins → build host closures (per-host pin-aware, see below) → inject `closureHash = basename(toplevel)` → stamp `meta.{signedAt, ciCommit, signatureAlgorithm}` → canonicalize via `nixfleet_canonicalize` → invoke a sign hook → write `releases/fleet.resolved.json{,.sig}`. The orchestration is a contract; the cache-push and signing tools it shells out to are not.

**Per-host commit pins (issue #88).** Each host entry MAY carry an optional `pin: { commit; reason; expiresAt? }` field declaring that the host's closure must be built from a specific source-control commit rather than the current release commit. mkFleet resolves pins from a most-specific-wins precedence chain (host > tag > channel) and emits the result on each affected host; `nixfleet-release` honors the pin by invoking `nix build "<pin_source_url>?rev=<commit>#nixosConfigurations.<host>.config.system.build.toplevel"` when the pin's commit differs from the release commit, and by filtering pins past `expiresAt` before the build dance starts. Operators MUST pass `--pin-source-url` to `nixfleet-release` whenever any active pin specifies a non-current commit (validated post-eval; missing flag aborts release with a list of offending hosts). Pin metadata reaches consumers via `hosts.<name>.pin` - the dashboard and CLI surface it for visibility.

**Producer hook contract (binding):**
- `--push-cmd` (optional) is invoked once per built closure with `cwd` = invocation cwd and these env vars set: `NIXFLEET_HOST` (host name), `NIXFLEET_PATH` (absolute store path), `NIXFLEET_CLOSURE_HASH` (basename of the path). Non-zero exit aborts the run.
- `--sign-cmd` (required) is invoked once with `NIXFLEET_INPUT` (path to a tempfile containing the canonical bytes) and `NIXFLEET_OUTPUT` (path the hook MUST write the raw signature bytes to). Non-zero exit, missing output file, or 0-byte output aborts the run.

These env-var names are part of the contract - renaming them is a §VIII amendment. The shell command strings themselves and any tools they shell out to (attic, nix copy, tpm-sign, cosign, GPG, ssh-keygen -Y, ...) are operator-supplied and not framework concerns.

### 2. Wire protocol (agent ↔ control plane)

| | |
|---|---|
| **Producer/Consumer** | Both agent and CP (Rust runtime) |
| **Schema** | v1 - RFC-0003 §4 |
| **Transport** | HTTP/2 over TLS 1.3, mTLS mandatory |
| **Version header** | `X-Nixfleet-Protocol: 1` |

**Evolution discipline.** Major version in header; mismatched major = HTTP 400. Additive fields within a major; MUST-ignore-unknown-fields on both sides. Removing a field requires a major bump and dual-version CP support during migration.

### 3. Probe descriptor

| | |
|---|---|
| **Producer** | `nixfleet-compliance` (Nix layer) |
| **Consumer** | Agent (Rust runtime) at runtime |
| **Schema** | Per-control `schema = "<framework>/<version>"` field (e.g. `"anssi-bp028/v1"`) |
| **Payload** | `{ command, args, timeoutSecs, expect, schema }` |

**Evolution discipline.** Each framework+version pair is immutable once shipped. New version = new schema string (`anssi-bp028/v2`); agent ships a handler registry keyed on `(control, schema)`. Controls MAY support multiple schema versions during migration.

### 4. Probe output

| | |
|---|---|
| **Producer** | Agent (executing the probe command) |
| **Consumer** | CP (aggregation), auditor (verification) |
| **Schema** | Declared by the control (§I.3 above) |
| **Canonicalization** | JCS |
| **Signature** | Host SSH ed25519 (see §II #4) |

**Evolution discipline.** Output shape is part of the control declaration - changes go through the control schema version. Signature covers the canonicalized bytes plus `{ control, schema, hostname, bootId, generationHash, ts }`.

### 5. Secret recipient list

| | |
|---|---|
| **Producer** | `fleet.nix` (Nix layer) |
| **Consumer** | agenix encryption tooling at commit time; agent at activation |
| **Schema** | agenix-native, pinned by `flake.lock` |

**Evolution discipline.** Pinned to the `agenix` version in `flake.lock`. Upgrading agenix is a coordinated commit that re-encrypts all secrets; treat as a spine-level change, not a routine dependency bump.

### 6. Log / event schema

| | |
|---|---|
| **Producer** | CP (reconciler), agent |
| **Consumer** | Operator queries, auditors reading historical state |
| **Schema** | RFC-0002 §7 - structured event with `logSchemaVersion` field |

**Evolution discipline.** Same as wire protocol - additive within a major, bump on breaking changes. Historical events MUST remain parseable for the declared audit retention window.

### 7. Activation timing invariant (fire-and-forget)

Operators tuning the magic-rollback confirm window MUST preserve the coupling:

```
confirm_deadline_secs ≥ POLL_BUDGET + CLOCK_SKEW_SLACK
```

Where:

- `confirm_deadline_secs` is per-channel `activate.confirmWindowSecs` in `fleet.resolved.json` (or the CP's `--confirm-deadline-secs` flag default - currently `360`).
- `POLL_BUDGET` is the agent-side fire-and-forget poll duration (currently `300s`, defined as `crates/nixfleet-agent/src/activation.rs::POLL_BUDGET`).
- `CLOCK_SKEW_SLACK` is the symmetric tolerance baked into both the freshness gate and the rollback timer (currently `60s`, defined as `crates/nixfleet-reconciler/src/verify.rs::CLOCK_SKEW_SLACK_SECS`).

**Why.** Agents activate via fire-and-forget: `systemd-run --unit=nixfleet-switch` queues a detached transient unit, and the agent then polls `/run/current-system` for up to `POLL_BUDGET`. If the deadline expires before the poll succeeds, the CP's rollback timer marks the `pending_confirms` row rolled-back and any subsequent confirm POST returns `410 Gone`, triggering the agent's local rollback path - *even though the activation itself was succeeding*. The slack absorbs benign clock drift between the CP's deadline computation and the agent's poll completion.

**How to tune.** Slow-link channels (large closures over residential uplinks, long activation scripts): raise `confirmWindowSecs` AND `POLL_BUDGET` together, keeping the inequality. Tight rollout windows (canary channels with short freshness): lower both, but never set `confirmWindowSecs < POLL_BUDGET + CLOCK_SKEW_SLACK`.

The CP enforces nothing here - operators that violate the invariant get the chaos cascade described above. Future versions may add a runtime warning at CP startup when `--confirm-deadline-secs` is below the documented minimum.

**Switch-inhibitor carve-out (issue #56).** The agent skips `switch-to-configuration` when a critical component (dbus implementation, systemd, kernel, init) differs between `/run/current-system` and the new closure - `nixos-rebuild switch` would refuse the same. The new generation is still bound to the system profile (`nix-env --set` runs unconditionally before fire), so the next reboot completes activation. The agent posts `ReportEvent::ActivationDeferred { component }` instead of running the live switch; this is NOT a `SwitchFailed` outcome and triggers no rollback.

The deferred lifecycle is **human-paced, not agent-paced**, so it explicitly opts out of the 360s confirm-deadline rollback timer documented above. CP receipt of `ActivationDeferred` parks the `host_dispatch_state` row in `DeferredPendingReboot`; the rollback timer's partial index `WHERE state = 'pending'` naturally excludes it. The confirm endpoint accepts post-reboot confirms against deferred rows without the deadline gate (`(Pending AND deadline > now) OR DeferredPendingReboot`). Wave promotion + channel-edge gates see deferred hosts as `ConfirmWindow` (in-flight, not terminal-for-ordering), so successor waves and channel crossings correctly wait for the operator's reboot.

Operator surfaces:
- `/v1/hosts` exposes `pendingReboot: true` for hosts whose `host_dispatch_state` row is `DeferredPendingReboot`. DB-backed, so the signal survives CP restart. Cleared when the row transitions to `Confirmed` (post-reboot retroactive confirm).
- `nixfleet status` shows `⟳ pending reboot` ahead of the `✓ converged` label so operators see deferred hosts at a glance.

Detection is canonicalize-equality on four store-relative paths: `etc/systemd/system/dbus.service`, `sw/lib/systemd/systemd`, `kernel`, `init`. Any mismatch defers; either side missing a path is out-of-scope and does not defer (see `crates/nixfleet-agent/src/activation/linux.rs::detect_switch_inhibitors`). The agent persists a `last_deferred` sentinel in its state-dir to suppress redundant activate-and-defer cycles for the same `closure_hash`; the suppression is cleared on `record_confirm_success` (post-reboot). Out of scope for this carve-out: glibc major-version swaps, `boot.loader.systemd-boot` ↔ `grub` swaps.

**Closure-hash quarantine carve-out (issue #55).** A second per-closure suppression sits alongside the deferred sentinel: when activation produces `SwitchFailed` or `VerifyMismatch` the agent records `last_failed_closure { closure_hash, last_failure_at, failure_count }` in its state-dir. On the next dispatch within `QUARANTINE_WINDOW_SECS` (24h) for the SAME closure_hash, the agent skips activate() and posts `ReportEvent::ClosureQuarantined` (rate-limited to one post per `QUARANTINE_REPOST_THROTTLE_SECS` = 1h). Auto-clears when the channel-ref advances to a fresher closure_hash (the suppression check stops matching). No CP-side state machine entry - the existing SwitchFailed → rollback flow already drives `host_dispatch_state` to RolledBack; quarantine is purely the operator-visible "agent has stopped retrying this closure" signal, surfaced as `quarantinedClosure: <hash>` on `/v1/hosts` and `✗ quarantined` in `nixfleet status`. The dispatch suppression order is: deferred first, then quarantine; both checks are O(1) state-dir reads with `closure_hash` equality, so dispatch overhead during steady-state suppression is negligible.

### 8. Rollout manifest

| | |
|---|---|
| **Producer** | CI (one manifest per channel, per `fleet.resolved` commit) |
| **Consumer** | Control plane (adoption + serve), agents (verify before consuming dispatch), auditors |
| **Schema** | v1 - shape defined in `nixfleet-proto::rollout_manifest`, semantics in RFC-0002 §4.4 |
| **Canonicalization** | JCS (RFC 8785), see §III |
| **Signature** | CI release key (see §II #1) - same trust root as `fleet.resolved.json` and `revocations.json` |
| **Identifier** | `rolloutId = sha256(canonicalize(manifest))`, hex lowercase. The hash IS the identity; see RFC-0002 §4.4. |
| **Anchor** | `fleetResolvedHash` - sha256 of the canonical bytes of the projecting `fleet.resolved.json`. Closes mix-and-match across snapshots at the same channel ref. |
| **Storage** | `releases/rollouts/<rolloutId>.{json,sig}` |

**Evolution discipline.** Within v1, fields may be added; consumers MUST ignore unknown fields. Adding a field changes every existing manifest's content hash by definition (the new field is part of the canonical surface), so `schemaVersion` bumps in lockstep with field additions that reach production CI - there is no "rolling additive" window for this artifact the way there is for `fleet.resolved.json`. Removing or changing the meaning of a field requires `schemaVersion: 2` and a migration window.

**Consumer MUST verify before use:**
1. JCS bytes match the canonicalized payload.
2. Signature verifies against the pinned `nixfleet.trust.ciReleaseKey`.
3. `(now − meta.signedAt) ≤ channel.freshnessWindow` (units: minutes; same gate as `fleet.resolved.json`).
4. Recomputed `sha256(canonical(received_bytes))` equals the `rolloutId` the recipient was told to fetch (rejects content-mismatched manifests). **The hash MUST be computed over the received bytes, not over a re-serialised parsed struct** - re-serialisation drops fields the verifier's proto doesn't know about, breaking content-addressing across additive schema changes and contradicting Pattern A's additive-evolution guarantee. The reconciler's `rollout_id_from_bytes` helper exists for this; producer-side `compute_rollout_id` (which has no received bytes, only the freshly-built struct) is the only legitimate caller of the parsed-struct path.
5. `meta.schemaVersion` is within the consumer's accepted range.
6. (Agent only) `(hostname, wave_index)` ∈ `manifest.host_set`.
7. (CP only, on adoption) `manifest.fleetResolvedHash` matches the hash of the `fleet.resolved.json` the CP currently holds verified - refuses adoption otherwise. Same rule: hash the received bytes, not the parsed struct.

**Producer pipeline (`nixfleet-release`).** Same orchestrator as `fleet.resolved.json` - after the resolved snapshot is signed, iterate `fleet.channels`, project each channel into a `RolloutManifest` (sorted `host_set`, target closure, wave layout, health gate, compliance frameworks, `fleetResolvedHash`), canonicalize, sign via the same `--sign-cmd` hook, write `releases/rollouts/<rolloutId>.{json,sig}`. The producer hook contract from §I #1 (`NIXFLEET_INPUT` / `NIXFLEET_OUTPUT` env vars) applies unchanged - one signing seam, three artifact types.

**Trust topology.** The CP holds NO signing key for rollouts. It is a verified stateless distributor: it adopts pre-signed manifests it can verify, refuses those it cannot, and serves the verified bytes byte-for-byte at `GET /v1/rollouts/<rolloutId>`. This preserves the "CP forges no trust" property: every byte an agent acts on traces back to a CI-held key.

**Architectural invariant - rollout topology is immutable for the rollout's life.** The manifest carries the resolved topology snapshot computed from `fleet.resolved` at projection time: wave membership (`host_set`), per-host target closure, AND per-budget host membership (`disruption_budgets[]`, the operator's selectors resolved against `fleet.hosts.tags` at that instant). Once the manifest is signed, none of these reshape until the rollout terminates and is replaced. Consequence: mid-rollout retags affect future rollouts only - they cannot reshape the budget enforcement an in-flight rollout is running under. This mirrors how waves already work and unifies the model: **`fleet.resolved` declares intent (selectors); the rollout manifest declares topology (resolved hosts).** Cross-rollout fleet-wide enforcement (e.g. "no more than one etcd node disrupted at a time, ever, across all channels") survives by matching budgets across active rollouts via selector equality.

---

## II. Trust roots

Four keys. Everything else is derived. For each: **who holds the private key, where the public key is declared, and who verifies.**

### 1. CI release key

| | |
|---|---|
| **Private** | HSM / TPM-backed keyslot on M70q (operator infra) |
| **Public (declared)** | `nixfleet.trust.ciReleaseKey` in `fleet.nix` (Nix layer) |
| **Verified by** | CP (on `fleet.resolved` load), optionally agents |
| **Algorithm** | `ed25519` **or** `ecdsa-p256` - declared alongside the public key; the signature's algorithm (§I #1 `meta.signatureAlgorithm`) must match |
| **Rotation grace** | `nixfleet.trust.ciReleaseKey.previous` valid for 30 days after rotation |

**Algorithm rationale.** ed25519 is the preferred default for HSMs, YubiKeys, cloud KMS, and software-held keys. ECDSA P-256 exists as a second-class citizen because commodity TPM2 hardware (Intel PTT, AMD fTPM, most discrete TPMs) exposes RSA + NIST P-256 but not the ed25519 curve (TPM2_ECC_CURVE_ED25519 = 0x0040 is rare). Both algorithms produce 64-byte signatures and have comparable security margins (~128-bit). Producers (lab CI) pick one at install time based on hardware; the trust-root declaration tells consumers which verifier to use.

**Public-key encoding.**
- `ed25519` - raw 32-byte public key, base64-encoded in `fleet.nix` (matches the format used by `ssh-keygen`, agenix, minisign).
- `ecdsa-p256` - uncompressed point, 64 bytes (`X ‖ Y`, no `0x04` prefix), base64-encoded. Consumers convert to SEC1 / DER SPKI at verify time.

The declaration shape:

```nix
nixfleet.trust.ciReleaseKey = {
  algorithm = "ecdsa-p256";  # or "ed25519"
  public    = "<base64 of raw bytes>";
};
```

**Signature encoding.** Raw 64 bytes for both algorithms - `R ‖ S` for ECDSA, standard `R ‖ S` for ed25519. No DER wrapping, no PGP armour. Put next to the canonical payload as `fleet.resolved.json.sig`.

**Rotation procedure.**
1. Generate new keypair (operator infra) - may differ in algorithm from the outgoing one.
2. Commit: set `ciReleaseKey = <new>`, `ciReleaseKey.previous = <old>` in `fleet.nix`. Consumers that pin both must accept signatures under either algorithm during the overlap.
3. CI starts signing with new key on next build.
4. After 30 days, remove `previous` from `fleet.nix`; old-key-signed artifacts rejected.

**Compromise response.** Immediate: remove compromised key from `fleet.nix`, set `rejectBefore = <timestamp>` (all artifacts signed before that are refused regardless of key). Rebuild CI environment. Sign a fresh fleet.resolved from known-clean CI. Document in `SECURITY.md`.

### 2. Cache trust keys

| | |
|---|---|
| **Private** | Each cache implementation's own keystore (harmonia signing key file, attic signing key, cachix authtoken-derived, etc.) |
| **Public (declared)** | `nixfleet.trust.cacheKeys` (Nix layer) - flat list of opaque strings |
| **Verified by** | nix's substituter (via `nix.settings.trusted-public-keys`) before every closure activation |
| **Format** | Implementation-defined string. Stock `<name>:<base64>` (harmonia, nix-serve, cachix) and attic's `attic:<host>:<base64>` are both accepted by nix and may be mixed in one list. |
| **Rotation grace** | Add the new key alongside the old in the list; remove the old once all hosts have switched. |

**Framework agnosticism.** The framework forwards these strings opaquely - it does not parse, dispatch on, or otherwise discriminate between cache implementations. Choosing harmonia, attic, cachix, plain `nix-serve`, or a custom HTTP cache is a fleet-side decision; the framework's only requirement is that the chosen impl serves the standard nix-cache HTTP protocol so that `services.nixfleet-cache.cacheUrl` works.

### 3. Org root key

| | |
|---|---|
| **Private** | Offline hardware (Yubikey) held by operator |
| **Public (declared)** | `nixfleet.trust.orgRootKey` (Nix layer) |
| **Verified by** | CP, when validating enrollment tokens |
| **Algorithm** | ed25519 |
| **Rotation grace** | 90 days; effectively never under normal operation |

**Rotation procedure.** Rare. If it rotates, every bootstrap token generated from the old key becomes invalid - every host re-enrollment requires a new token signed by the new key. Not a routine event.

**Compromise response.** Catastrophic: every enrollment token is potentially forgeable. Revoke old key, issue all hosts new bootstrap tokens, re-enroll fleet. Consider this the equivalent of an "infrastructure rebuild" event.

### 4. Host SSH key

| | |
|---|---|
| **Private** | Per-host `/etc/ssh/ssh_host_ed25519_key` (generated at provision) |
| **Public (declared)** | `fleet.nix` host entry (`hosts.<n>.pubkey`) (Nix layer) |
| **Verified by** | Auditor (probe output signatures), CP (mTLS cert binding at enrollment) |
| **Algorithm** | ed25519 (OpenSSH-compatible) |
| **Rotation grace** | Host key change = re-enrollment; no grace |

**Rotation procedure.** If a host's key changes, the old host is considered gone and a new one is being enrolled. Secrets must be re-encrypted for the new recipient; probe-output signatures chain through the boot/generation record.

### Operational note: enforce the trust posture with `strict` mode

The four roots above describe **what** the framework verifies. **Whether** that verification fires depends on the CP being configured to use it: `--client-ca` enables mTLS, `--revocations-{artifact,signature}-url` keeps revocations live across rebuild, and the `X-Nixfleet-Protocol` header guards the wire shape. By default each fallback degrades silently (warn-and-continue) so dev/test isn't blocked.

Production fleets should set `services.nixfleet-control-plane.strict = true` (or pass `--strict` / `NIXFLEET_CP_STRICT=1`). In strict mode the CP refuses to start when any of these flags is unset, and rejects requests missing the protocol header. The NixOS module emits a warning when the listener is exposed beyond loopback while `strict = false`.

---

## III. Canonicalization

**JCS (RFC 8785) with a single Rust implementation, byte-identical across all signers and verifiers.**

Producer-side (the Nix layer's `lib/mk-fleet.nix`) MUST emit values that round-trip through JCS losslessly: ints only (no floats), deterministic attr order, no JSON-incompatible types. Consumer-side (the Rust runtime's `bin/nixfleet-canonicalize`) pins the library.

- **Library choice.** Pinned to [`serde_jcs`](https://crates.io/crates/serde_jcs) `0.2`, hosted by `crates/nixfleet-canonicalize`. Rationale: direct RFC 8785 implementation over `serde_json::Value`; handles UTF-16 key sorting and ECMAScript number formatting per spec. Any change to this pin is a contract change (§VII) requiring signoff from every layer that signs or verifies artifacts (CI/infra, Nix, Rust).
- **Golden-file test.** `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.{json,canonical}` with byte-exact equality asserted in `tests/jcs_golden.rs`. Runs on every push via pre-push `cargo nextest run --workspace`; fails loudly on any drift. The ed25519-signed-bytes extension of this fixture lands alongside the CI release key.
- **Usage.** Every signed artifact (fleet.resolved, probe output) is canonicalized via this single library before signing and before verification. No ad-hoc serializers in Nix, shell, or other crates.

When the Nix layer needs to produce a JCS-canonical artifact (e.g. CI signing fleet.resolved), it invokes the same Rust canonicalizer via a small shell tool (`nixfleet-canonicalize`). Do not reimplement in Nix or shell.

---

## IV. Control-plane storage purity rule

The control plane's SQLite database exists to cache operational state. Every column MUST satisfy one of:

1. **Derivable from git + agent check-ins.** Documented in a line comment on the column:
   ```sql
   CREATE TABLE hosts (
     hostname TEXT PRIMARY KEY,          -- derivable from: fleet.resolved
     current_gen TEXT,                   -- derivable from: agent check-in
     last_seen_at DATETIME,              -- derivable from: agent check-in
     ...
   );
   ```
2. **Explicitly listed in "accepted data loss."** See below.

**Accepted data loss list** - state that is intentionally not preserved through a control-plane teardown:

| State | Reason | Recovery |
|---|---|---|
| Certificate revocation history | Revocations are operational decisions, not automated. | Operator re-declares revocations after teardown. |
| Per-rollout event log (> 30 days old) | Historical trace, not operational. | Available via log aggregation (§I.6), not CP-internal. |

**Rule.** A new column that is neither derivable nor on the accepted-loss list is a contract violation. It fails the teardown test (`#14`) and must be either removed or moved into the declarative state.

---

## V. Versioning patterns

### Current state at a glance

| Contract | Current version | Evolution |
|---|---|---|
| `fleet.resolved.json` | `schemaVersion: 1` | Additive within v1; bump for breaking changes. `meta.signatureAlgorithm` added in v1 - optional, defaults to `"ed25519"` when absent. |
| `RolloutManifest` | `schemaVersion: 1` | Additive within v1; every shape change rebumps in lockstep (§I #8). |
| `revocations.json` | `schemaVersion: 1` | Same shape as `fleet.resolved.json`. |
| Wire protocol | v1 (header) | Additive within major; dual-support during migration |
| Probe descriptor per framework | `<framework>/v1` per framework | New string for new shape; old shape kept during migration |
| Probe output | Tracked with the control | Same as descriptor |
| Log/event | `logSchemaVersion: 1` | Same pattern as wire protocol |
| Agenix format | Pinned by `flake.lock` | Treat upgrade as spine change |

### The three patterns and why they are NOT unified

Three boundary contracts in this framework version themselves three different ways. Each is right in its scope; trying to unify them would lose information.

| Pattern | Used by | Identifier | Scope of a bump |
|---|---|---|---|
| **A - `meta.schemaVersion: u32`** | Signed artifacts (`fleet.resolved.json`, `RolloutManifest`, `revocations.json`) | JSON field inside the artifact | The data shape of the artifact bytes |
| **B - HTTP header** | Agent ↔ CP wire (§I #2), log/event schema (§I #6) | `X-Nixfleet-<Capability>: <major>` per request | Per-request interaction capability |
| **C - Embedded schema string** | Compliance controls (§I #3, #4) | `<vocabulary>/<version>` per item | One vocabulary item's contract |

Why not pick one across the board:

- **Wire version inside the artifact** (Pattern A everywhere) would couple wire bumps to artifact bumps - every new wire field would force re-signing every artifact in flight, even artifacts whose data shape did not change.
- **Artifact schema in an HTTP header** (Pattern B everywhere) would destroy self-description: an auditor reading canonical bytes off disk, out of a cache, or shipped by email has no HTTP envelope to read the version from.
- **Global vocabulary version** (Pattern C everywhere) would force every contract to re-version on any single change, breaking the per-framework / per-artifact cadence assumption that lets compliance frameworks evolve independently.

The patterns are right in their respective scopes; the inconsistency is real but load-bearing.

### Decision tree - picking a versioning pattern for a new contract

**Q1.** Is the contract a self-describing chunk of data that may be read out-of-context (off disk, out of a cache, by an auditor, by a third-party tool, mailed to someone)?
- **Yes** → **Pattern A** (`meta.schemaVersion: u32`). The bytes carry their own version label.

**Q2.** Is the contract a per-request interaction capability between two endpoints sharing live session state?
- **Yes** → **Pattern B** (HTTP header). The version applies to the request, not to a persisted blob.

**Q3.** Is the contract an independent vocabulary item that evolves on its own cadence, distinct from peer items in the same family?
- **Yes** → **Pattern C** (embedded schema string). Each item carries its own version; the family does not aggregate.

If none fit, the contract is probably small enough not to need a versioning mechanism at all - pin by `flake.lock` (e.g., agenix format §I #5) or by review.

### Naming conventions per pattern

| Pattern | Field/key naming | Version literal | Example |
|---|---|---|---|
| A | camelCase JSON keys; envelope under `meta.*` | unsigned integer | `"meta": {"schemaVersion": 1}` |
| B | `X-Nixfleet-<Capability>` HTTP header | bare integer for major | `X-Nixfleet-Protocol: 1` |
| C | `<vocabulary>/<version>` string | `v<N>` suffix | `"anssi-bp028/v1"` |

### Bump procedure per pattern

**Pattern A - `meta.schemaVersion`.**

1. Additive fields land within the current `schemaVersion`; consumers MUST ignore unknown fields.
2. Removing or changing the meaning of a field requires bumping `schemaVersion` and shipping a migration window where consumers accept both versions.
3. Compatibility window default: 30 days from the first signed artifact under the new version. After the window, old-version artifacts are refused.
4. Sunset notice: announce the bump in `CHANGELOG.md` at least one release before the cutover; flag old-version artifacts in CP logs during the window.
5. Producers stop emitting the old version at least one full `freshnessWindow` before consumers stop accepting it, so no in-flight artifact ages out mid-rotation.

**Pattern B - HTTP header.**

1. Additive fields within the same major (consumers MUST ignore unknown fields, same posture as Pattern A).
2. Removing a field or changing semantics requires a major bump (`X-Nixfleet-Protocol: 2`) and dual-version CP support during migration.
3. Compatibility window default: one full agent renewal cycle (30 days) so a rolling cert renewal naturally drags every agent onto the new major.
4. Sunset notice: CP logs a deprecation warning when it admits a request under the old major; flips to HTTP 400 after the window.
5. Wire endpoints are versioned, not rotated - there is no "old key" analogue; the deprecation is the rotation.

**Pattern C - embedded schema string.**

1. Each `<vocabulary>/<version>` pair is immutable once shipped.
2. New version = new schema string (e.g. `anssi-bp028/v2`); the agent ships a handler registry keyed on `(control_id, schema)`.
3. Compatibility window: controls MAY support multiple schema versions during migration; the framework imposes no global cutover.
4. Sunset notice: per-control, declared by the framework's release notes when a new schema version supersedes an old one.
5. Rotation/deprecation is per-control; the framework does not aggregate across the vocabulary family.

### Concrete lifecycle examples

**Pattern A - adding `meta.signatureAlgorithm` to `fleet.resolved.json`.** Field added optionally within `schemaVersion: 1`. Consumers absent the field interpret it as `"ed25519"` for backward compatibility (§I #1). No bump. This is the prototypical compatible additive change - under stricter discipline it could have been a `schemaVersion: 2` bump, but the "default when absent" rule preserved single-version compatibility for unmodified consumers.

**Pattern B - widening `EvaluatedTarget` (RFC-0003 §4.1).** Three new optional fields (`rollout_id`, `wave_index`, `activate`) added to `CheckinResponse.target`. Per RFC-0003 §6 additive rule, no `X-Nixfleet-Protocol` bump required: old agents that don't deserialize the new fields keep working; new agents reading them from an old CP receive `None`.

**Pattern C - adding a new compliance framework version.** A new `anssi-bp028/v2` lands as a new probe descriptor. The agent registry adds a v2 handler keyed on `(control_id, "anssi-bp028/v2")`. Hosts on channels still emitting v1 probes keep the v1 handler; hosts on channels migrated to v2 receive v2 probes. No global cutover; the v1 handler is removed from the registry only after every channel's compliance config is migrated.

---

## VI. Implementation agnosticism

The framework promises *mechanism*, not *implementation*. The following are explicit non-commitments - the framework runtime contains no code that depends on these choices, and a fleet may freely substitute any conforming alternative without forking nixfleet.

| Concern | Framework requires | Fleet picks |
|---|---|---|
| **GitOps source** for the channel-refs poll | An HTTPS URL pair (artifact + signature) that returns the raw signed bytes when GET'd, optionally with `Authorization: Bearer <token>`. Configured via `services.nixfleet-control-plane.channelRefsSource.{artifactUrl, signatureUrl, tokenFile}`. | Forgejo / Gitea / GitHub / GitLab / sourcehut / plain HTTPS / S3 with presigned URLs / anything HTTP-shaped. URL templates for common forges live in `flake.scopes.gitops.*` (this repo's `impls/gitops/`) as pure data - adding a new forge is one `.nix` file, no Rust changes. |
| **Binary cache server** | Nothing - the framework does not ship a cache-server module. Hosts that should serve a cache wire one in fleet-side. | `services.harmonia`, `services.atticd`, `services.nix-serve`, cachix as a service, or a hand-rolled wrapper. The consuming fleet picks. |
| **Binary cache client** | An HTTPS URL + a public key string. Configured via `services.nixfleet-cache.{cacheUrl, publicKey}`. | Any cache speaking the standard nix-cache HTTP protocol (narinfo + nar). Identical client config regardless of which server impl is upstream. |
| **Cache trust keys** | A flat list of opaque strings forwarded to `nix.settings.trusted-public-keys`. Configured via `nixfleet.trust.cacheKeys`. | Stock `<name>:<base64>`, attic `attic:<host>:<base64>`, or both at once - see §II #2. |
| **PKI / mTLS issuer** | Cert + key file paths on disk. The framework reads them; their provenance is not a contract. | Caddy's internal CA (current fleet choice), Smallstep, vault-pki, hand-rolled scripts, or a public CA - anything that produces RSA / ECDSA / Ed25519 cert files compatible with rustls. |
| **Secrets backend** | Cert / key / token *paths* in option fields. The framework reads files; how they got there is not a contract. | agenix (current fleet choice), sops-nix, plain nixops, manual secret-staging scripts, or systemd-creds. |
| **Disk layout** | A `disko.devices` attrset on the host. | Hand-rolled disko config in the consuming fleet, or none if filesystems are pre-provisioned. |
| **Impermanence** | An `environment.persistence` option must exist (the framework's own service modules contribute to it). The framework imports the upstream `impermanence` flake to satisfy this. | Activate via `nixfleet.impermanence.enable = true`, or leave disabled - the schema is always declared. |

**What this means for fleets.** Every framework binary or NixOS module touches only the contract surface above. A fleet that wants GitHub instead of Forgejo, harmonia instead of attic, sops-nix instead of agenix, or vault-pki instead of Caddy CA changes its scope imports and its option values - the framework code is rebuilt without modification.

**What this means for nixfleet maintainers.** New tech-specific impls land under `impls/<family>/` and get exposed at `flake.scopes.<family>.<impl>` - they remain opt-in for fleets. If something tech-specific *must* enter the framework's runtime path - e.g. a new wire-protocol participant - it's a contract change governed by §VII below.

### Irreducible technology assumptions

A small set of technology choices are **load-bearing** for the framework - they're not implementation choices a fleet can swap. Replacing one of these means building a different framework.

| Assumption | Why load-bearing | Replacing means |
|---|---|---|
| **Nix + flakes** | The whole declarative side (mkHost, mkFleet, the option system, hostSpec contract, fleet.resolved evaluation) is built on Nix evaluator semantics; the framework has no non-Nix front-end. | Re-implementing the declarative layer in another DSL - different framework. |
| **NixOS** (system layer) | The Linux agent's activation pipeline assumes NixOS' generation model: `/run/current-system` resolves to the active toplevel, `nixos-rebuild switch --system <path>` is the activation primitive, post-switch verification reads `basename(realpath /run/current-system)`. The §I #1 contract refers to "closure hash"; that concept is meaningful in NixOS terms. | A separate activation backend abstraction - see roadmap. Until that lands, non-NixOS Linux is out of scope. |
| **systemd** | Every framework NixOS module declares `systemd.services.nixfleet-*`. Hardening, restart policy, credential plumbing, dependency ordering all use systemd primitives. | Rewriting the system-service layer for runit/s6/launchd - same scope as a non-NixOS port. |
| **mTLS over HTTP/1.1** | Agent ↔ control-plane authentication identity is the client cert CN; authorisation is per-route. The CP's rustls config is the trust boundary the agent verifies; replacing TLS means a different wire protocol. | A different wire protocol (Noise, Tailscale ACL, mutual auth over WireGuard). Different framework. |

**TPM is *not* on this list.** TPM hardware is a *fleet's choice* of signing keyslot, not a framework requirement. The `keyslots/tpm` impl ships at `flake.scopes.keyslots.tpm` as one option among many; the framework runtime never links a TPM library. A fleet using a YubiKey, software key, HSM, or KMS for the CI release key is fully framework-supported - see §I #1's hook contract. The current reference fleet happens to use TPM-backed ECDSA P-256; that's deployment opinion.

**Why call these out.** The agnosticism work made it easy to add new tech-specific impls as scopes. The four assumptions above cannot be captured by the same pattern - there is no scope a fleet can import to replace systemd. Documenting them here prevents the framework from drifting into pretending they're substitutable, and gives future maintainers a clear test: *if it's listed below the agnosticism table, scope-side; if it's listed in this irreducible-assumptions table, framework-side and out of scope to abstract.*

---

## VII. Non-contracts (explicit)

The following are NOT contracts - they may change without coordination:

- Internal CP SQLite layout (as long as §IV rule holds).
- Internal agent process structure (threads, tokio tasks).
- Internal reconciler intermediate data structures.
- Nix module option defaults (overridable per-host).
- Formatter choices, lint rules.
- Directory layout inside `crates/` beyond crate names.

If something that should be a contract is drifting, propose it as an addition to this document via PR - do not unilaterally stabilize it in code.

> **Implementation status disclosure.** Some contracts in §I - notably parts of `CheckinResponse.target` (RFC-0003 §4.1, tracked in #68) and the rollback-and-halt semantics in the reconciler (RFC-0002 §5.1, tracked in #69) - are **schema-honored but behavior-partial**. The framework declares the wire shape and the option surface, but specific code paths are deferred. This disclosure is not a contract weakening - the listed contracts remain authoritative and additive - but it makes explicit that "passes verification" does not yet mean "exercises every documented field."

---

## Operator config file

`~/.config/nixfleet/config.toml` is operator-side state with the following schema:

```toml
cp_url      = "https://cp.example.com:8080"
ca_cert     = "/etc/nixfleet/ca.pem"
client_cert = "/home/operator/.config/nixfleet/operator.pem"
client_key  = "/home/operator/.config/nixfleet/operator.key"
```

All fields are optional in the file - missing fields fall through to `NIXFLEET_*` env, then to explicit flags. The CLI fails closed: any unfilled field triggers `ConfigError::Missing` with a hint to run `nixfleet config init`.

The file path can be overridden per-invocation with `--config <path>` or `NIXFLEET_CONFIG`.

---

## VIII. Amendment procedure

1. Open a PR that modifies this document.
2. Label it `contract-change`.
3. Review requires a signoff from each layer whose code implements the contract.
4. Merge only after the code change that implements the new contract is ready in the same PR (or a linked follow-up that must land within the same spine milestone).

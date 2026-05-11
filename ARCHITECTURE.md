# nixfleet architecture: declarative, signed, sovereign

**Design principle.** The control plane is a caching router for signed declarative intent. It holds no secrets, forges no trust, and can be rebuilt from empty state without data loss.

Every structural decision below serves that inversion of trust. In today's nixfleet, the control plane is the source of truth — compromise it, and the fleet follows wherever it points. In this design, truth lives in git and in signing keys; the control plane only moves already-signed artifacts around. Destroying the control plane is an outage, not a breach. Rebuilding it from the flake and the signed artifacts in storage gives you back the same fleet.

This document consolidates the v0.2 design: the spine, the RFCs, the Rust/Nix boundary, the content-addressing generalization, and the supporting homelab infrastructure into a single architecture with a single build order.

---

## 1. Components

Eight components, each with a defined role, a defined owner, and a defined trust property. Components only interact through versioned, typed boundaries.

### 1.1 The flake (source of truth)

Git-tracked, hosted on a self-run Forgejo instance on the M70q. Contains:

- `nixosConfigurations.<host>` — per-host NixOS modules.
- `fleet` flake output — produced by `mkFleet { ... }` per RFC-0001; describes hosts, tags, channels, rollout policies, edges, disruption budgets.
- `age.secrets.<name>` — secrets encrypted per-recipient at rest, declared alongside the fleet.
- `nixfleet.compliance.controls.<name>` — typed controls with static `evaluate` and runtime `probe` projections.

Trust role: **primary trust root for intent.** A commit that passes review IS the desired state. No other place in the system can claim "the fleet should be X" without a corresponding commit.

### 1.2 Continuous integration (the intent-signing oracle)

Runs on the M70q (Hercules CI agent, or Forgejo Actions with a self-hosted runner). On every commit to a watched branch:

1. Evaluates the flake; builds every host's closure.
2. Runs static compliance gates (`type = static` controls evaluated against each `config`). Failure aborts the pipeline; no release is produced.
3. Pushes closures to attic, which signs them with its ed25519 private key.
4. Produces `fleet.resolved.json` (RFC-0001 §4.1 projection) and signs it with the CI release key.
5. Updates channel pointers (`stable`, `edge-slow`, …) to the new git ref, committing the signed artifact set.

Trust role: **converts reviewed-and-merged commits into signed releases.** CI key lives in an HSM, ideally on the M70q with a TPM-backed keyslot. Rotation is a documented procedure, not an incident response.

### 1.3 Attic binary cache

Runs on the M70q. Stores every closure CI produces, content-addressed by `sha256`, signed with its own ed25519 key. Clients verify signatures against a pinned public key embedded in their NixOS config.

Trust role: **self-verifying content store.** A compromised attic host cannot forge closures: the signing key is the trust root, not the host. An attacker who steals attic's disk learns what closures have been built; they cannot inject malicious ones into any host.

### 1.4 Control plane (the router)

Rust/Axum service, SQLite for operational state, mTLS for all incoming connections. **What it does:**

- Polls the git forge for channel-ref updates (or receives webhooks).
- Fetches the signed `fleet.resolved.json` for each channel rev; verifies the CI signature; if it doesn't verify, refuses to reconcile.
- Runs the reconciler (RFC-0002 §4 decision procedure) on each tick.
- Serves agent check-ins (RFC-0003): tells each host its current target closure hash, current rollout membership, expected probes.
- Records observed state (last check-in, current generation, probe results) as a cache of what agents have reported.

**What it does not do:**

- Hold any secret material (all secrets are agenix-encrypted in the flake).
- Sign anything that a host is asked to trust (closures → attic; intent → CI; probe outputs → hosts).
- Store anything that cannot be recomputed from git + attic + agent check-ins.

Trust role: **router.** Compromise yields at worst a denial of service (refuse to propagate updates) or a replay attack (point hosts at stale-but-valid closures). Cannot inject code, cannot read secrets, cannot forge compliance evidence.

Destroying the control plane and rebuilding from scratch: re-pull fleet.resolved from git, re-fetch channel refs, let agents check in on their next poll cycle. Operational state reconstructs within one reconcile tick per channel.

### 1.5 Agent (the actuator)

Rust daemon running on every managed host. Single-binary, minimal dependencies. **What it does:**

- Polls the control plane over mTLS at the channel's declared cadence.
- On a new target: fetches the closure from attic (not from the control plane), verifies attic's signature, verifies the hash.
- Decrypts host-scoped secrets from the flake using the host's private ed25519 (SSH host key).
- Runs `nixos-rebuild switch`. Opens the magic-rollback confirm window.
- On post-activation boot: phones home with `bootId` + probe results. On silence past the window: auto-rollback.
- Reports current generation + probe outcomes at next check-in.

**What it does not do:**

- Accept arbitrary commands from the control plane. The vocabulary is only "your target is closure `sha256-X`". Not "run this shell snippet", ever.
- Trust the control plane's closure recommendation without signature verification against attic's pinned key.
- Hold long-lived credentials beyond its mTLS client cert (short-lived, auto-rotating) and its SSH host key (machine-lifetime).

Trust role: **local decision-maker.** The agent is the last line of defense against a compromised control plane. If signatures don't verify, it refuses. If the magic-rollback window closes silently, it reverts. Every decision is made with information the agent can independently verify.

### 1.6 Compliance framework (enforceable evidence)

`nixfleet-compliance` repo. Controls declared as typed units with two projections:

- `evaluate :: config → { passed, evidence }` — pure, runs at CI time. Violations fail static gate; no release produced.
- `probe :: { command, expectedShape, schemaVersion }` — descriptor consumed by the agent post-activation. Output is canonicalized and signed by the host's key, producing non-repudiable evidence.

Every control belongs to one or more frameworks (ANSSI-BP-028, NIS2, DORA, ISO 27001). A channel's `compliance.frameworks` list enforces the union of controls.

Trust role: **turns NixOS configuration into auditable, content-addressed evidence.** The chain: host key signs probe output → closure hash pins what was running → git commit pins what was intended. An auditor verifies the whole chain without trusting the control plane, the CI runner, or the operator.

### 1.7 Secrets (zero-knowledge ferrying)

agenix-style: secrets encrypted per-recipient in git. Recipients are host SSH pubkeys, declared in `fleet.nix` under `secrets.<name>.recipients`. Ciphertext ships as part of the closure or as separate content-addressed blobs. Decryption happens on the target host, using its private SSH host key, into tmpfs only.

Trust role: **eliminates the control plane from the secret path entirely.** A fully-public flake repo combined with good host key hygiene gives you the same secrecy guarantees as a locked-down vault. Rotation = re-encrypt + commit + redeploy.

### 1.8 Test fabric (microvm.nix)

In-flake fixture. Each scenario declares N microvms (cloud-hypervisor, shared Nix store via virtiofs), a stub control plane, and an expected action plan. Exercises: clean rollout, canary rollback on probe failure, agent offline during rollout, host key rotation, cert revocation, compromised-control-plane simulation (swap signing key, verify hosts refuse).

Runs in `nix flake check` on PR for small scenarios (10 hosts); nightly for larger (50).

Trust role: **the only honest way to know the protocol is correct.** Every state machine in RFC-0002 must have fixture coverage. No transition lands without a test that exercises it. The reconciler is a pure function (§2 below); there's no excuse for not testing it exhaustively.

---

## 2. The Nix / Rust boundary

**Nix owns evaluation.** `mkFleet`, selector algebra, compliance control declarations, secret recipient lists. Produces signed artifacts at CI time. Never called at runtime.

**Rust owns execution.** Reconciler, state machines, agent protocol, activation, probe running, CLI. Takes signed artifacts as input; never evaluates Nix.

**Boundaries.** Three typed, versioned contracts:

1. `fleet.resolved.json` — Nix → Rust, via CI, signed.
2. Compliance probe descriptors — Nix → Rust, embedded in closures, schema-versioned.
3. Agent/control-plane wire protocol — Rust ↔ Rust, versioned in header.

Crossing a boundary always means a version check and a signature verification (where applicable). Nothing is trusted by proximity.

---

## 3. The main flow

The happy path, one commit from push to all hosts converged:

```
1. operator ─── git push ──────────────▶ Forgejo
                                             │
2. Forgejo ─── webhook ────────────────▶ CI
                                             │
3. CI evaluates flake → builds closures per host
   CI runs static compliance gate
   CI pushes closures → attic (signs)
   CI produces fleet.resolved.json (signs)
   CI updates channel pointer, commits
                                             │
4. control plane polls/receives ◀───── git ref change
   verifies fleet.resolved signature
   reconciler emits action plan for new rollout
                                             │
5. agent (workstation, canary wave) polls ─▶ control plane
   control plane replies: target = sha256-X, rollout R, wave 0
                                             │
6. agent fetches sha256-X from attic
   verifies attic signature, verifies hash
   decrypts host-scoped secrets locally
   activates → confirm window opens
                                             │
7. agent boots new generation
   runs runtime probes, signs outputs with host key
   phones home /agent/confirm with boot ID + probe results
   control plane accepts confirmation
                                             │
8. soak elapses → wave 0 promoted → wave 1 begins
   m70q-attic receives dispatch; same sequence
                                             │
9. wave 1 converges → rollout Converged
   channel's lastRolledRef updated to new rev
```

Nothing in this flow requires trusting the control plane with anything it shouldn't have. The control plane knows: which hosts exist, which closure hash each should run, which rollouts are in flight, what check-ins have happened. It does not know: what's in the closures, what's in the secrets, whether the probe outputs were forged (it can verify via host keys, but it could not fabricate them).

---

## 4. The trust flow

Independent of the operational flow, trace where trust *originates* and where it's *verified*. This is the diagram that should stay true forever:

```
trust origins (signing keys, offline, rotatable):

   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
   │  CI release key │   │  attic cache key│   │  org root key   │
   │  (signs fleet.  │   │  (signs closures│   │  (signs bootstrap│
   │   resolved)     │   │                 │   │   tokens)       │
   └────────┬────────┘   └────────┬────────┘   └────────┬────────┘
            │                     │                     │
            │                     │                     │
trust per-host (derived, short-lived):
            │                     │                     │
            │            ┌────────┴────────┐            │
            │            │  host SSH key   │            │
            │            │  (signs probe   │            │
            │            │   outputs,      │            │
            │            │   decrypts      │            │
            │            │   secrets)      │            │
            │            └────────┬────────┘            │
            │                     │                     │
            │            ┌────────┴────────┐            │
            │            │  agent mTLS cert│            │
            │            │  (short-lived,  │            │
            │            │   derived from  │            │
            │            │   host key at   │            │
            │            │   enrollment)   │◀───────────┘
            │            └─────────────────┘
            │
verification happens everywhere (runtime, cheap):

   agents verify attic signatures on every closure fetch.
   agents verify CI signatures on every fleet.resolved fetch (if fetched directly).
   control plane verifies CI signatures before reconciling new revisions.
   control plane verifies agent mTLS certs on every check-in.
   auditors verify host-key signatures on probe outputs post-hoc.
```

Four keys. Everything else is derived. Compromise of any derived credential has a bounded blast radius because the roots are separate.

---

## 5. The failure cases

The design earns its keep when things go wrong. Walking through the scenarios:

**Control plane host is compromised** (attacker has root on the VM hosting Axum/SQLite). Attacker cannot: read secrets, forge closures, inject malicious code. Can: refuse to serve updates (DoS), serve stale-but-valid targets (replay). Mitigation: agents refuse to accept targets older than a configurable freshness window signed by CI.

**Attic cache host is compromised.** Attacker cannot forge closures (signing key is the trust root). Can: delete closures (hosts fall back to building locally if builders are present, else stall). Can: learn what closures exist (metadata leak). Disk loss is recoverable from CI artifacts.

**CI runner is compromised.** Serious — attacker can sign releases. Mitigation: CI key in HSM, CI runner in restricted environment, signing operation requires hardware confirmation. Detection: anomalous release signatures (signed outside normal CI run time) trip alerts. Recovery: revoke CI key, re-sign from clean environment, all agents refuse old-key artifacts.

**Host is compromised (root on the target machine).** Attacker can: read secrets decrypted for that host, forge probe outputs signed with that host's key. Cannot: affect other hosts, modify the control plane's view of the fleet. Detection: probe outputs from a compromised host might show inconsistencies that trigger runtime gates. Mitigation: TPM-backed host keys make key extraction hard; short-lived agent mTLS certs limit persistence.

**Operator is compromised / malicious.** If they have git commit access: can push any config. Mitigation: protected branches, mandatory review, CI static compliance gate catches obviously-bad configs (SSH password auth, disabled firewall, etc.) before release. Post-hoc: git history is the audit log.

**Network partition mid-rollout.** Agents cache last known desired state, continue operating. Magic rollback handles post-activation failures locally. Rollout pauses until partition heals; disruption budgets prevent cascade.

---

## 6. What to build, in order

Ten phases. Each phase produces a deliverable that can be tested and demonstrated before the next phase starts.

### Phase 0 — The M70q as coordinator

Prerequisite for everything. On the M70q: NixOS with flakes, agenix for secrets, Caddy + Tailscale for access control, Forgejo for git hosting, attic for binary cache, Hercules CI agent (or Forgejo Actions runner) for builds, Restic for backups. All declarative, all in a single `m70q-attic.nix` module.

Deliverable: a git push to Forgejo triggers a build, produces cached closures, and updates a channel pointer. No fleet yet. Just the CI spine.

### Phase 1 — `mkFleet` and `fleet.resolved`

Ship the Nix module from RFC-0001. Declare your actual fleet (m70q, workstation, rpi-sensor) in a `fleet.nix`. Add `fleet.resolved` as a flake output. Extend CI to produce and sign `fleet.resolved.json` alongside closures.

Deliverable: `nix eval .#fleet.resolved --json` produces a valid signed artifact committed by CI.

### Phase 2 — Reconciler prototype (read-only)

Ship the Rust reconciler from the spike. Runs as a systemd timer on the M70q. Reads `fleet.resolved.json`, reads a simulated `observed.json` (no agents yet), prints the action plan to the journal. No actions taken — just planning.

Deliverable: every commit produces a visible plan in the journal. Operator can review what *would* happen.

### Phase 3 — Agent skeleton (pull-only, no activation)

Rust daemon on each host. Polls control plane over mTLS. Reports current generation at each check-in. Does not activate anything yet — the control plane logs intended targets, the agent logs what it was told, but no `nixos-rebuild` runs.

Deliverable: each host correctly reports itself. Control plane correctly computes deltas. Enrollment flow (bootstrap token → cert) works end-to-end.

### Phase 4 — Activation + magic rollback

Agent gains the ability to run `nixos-rebuild switch --flake git+https://...#<hostname>`. Post-activation confirm window. Auto-rollback on silence. Closure fetch from attic with signature verification.

Deliverable: a git commit causes workstation to upgrade, then m70q, respecting canary wave ordering. Intentionally breaking the post-activation handshake (e.g. agent refuses to phone home) causes the host to revert within the window.

### Phase 5 — Secrets via agenix

Migrate any runtime secrets (Restic repo keys, API tokens, etc.) into agenix. Control plane never sees them. Demonstrate rotation: change a secret in the flake, commit, observe re-encryption and re-deployment without control-plane involvement.

Deliverable: `tcpdump` on control plane ↔ agent shows no secret material during any rollout.

### Phase 6 — Compliance gates (static)

Port `nixfleet-compliance` controls to the typed model. Wire CI to run static gates. Intentionally commit a config that violates ANSSI-BP-028 (e.g. SSH password auth on): CI refuses to produce a release.

Deliverable: bad configs never reach production. Audit trail shows which control caught which violation, in git history.

### Phase 7 — Compliance gates (runtime) + signed probe outputs

Agent runs probes post-activation, canonicalizes output, signs with host key. Control plane aggregates. Runtime gate blocks wave promotion on probe failure.

Deliverable: end-to-end signed evidence chain for an ANSSI audit. Given a host, produce: its current closure hash, the closure's inputs from git, the probe outputs for the running generation, all cryptographically linked.

### Phase 8 — microvm.nix test fabric

Fleet simulation. Every state machine in RFC-0002 covered by at least one fixture scenario. Negative tests for every signature verification. Run in `nix flake check` on PR.

Deliverable: regression protection. Refactoring the reconciler's state machines doesn't accidentally ship a week later as a production incident.

### Phase 9 — Declarative enrollment

Bootstrap tokens signed by org root key, scoped to expected hostname + pubkey fingerprint. `nixos-anywhere` + token yields a fully-enrolled host with no operator clicks after the initial provision.

Deliverable: adding a new RPi sensor is: `nix run .#provision rpi-sensor-02 <mac>` + PR adding its entry in `fleet.nix`. Nothing else.

### Phase 10 — Control-plane teardown test

The actual validation of the design principle. Destroy the control plane's SQLite. Restart it from empty state. Observe: it re-reads fleet.resolved + revocations.json from git, replays the verified revocation set, accepts agent check-ins, reconstructs fleet state within one reconcile tick per channel. No data lost.

Deliverable: a documented "disaster recovery" procedure that takes under 5 minutes from healthy-control-plane-gone to full-fleet-visibility restored.

#### CP-resident state by recovery profile

Every SQLite table the CP keeps falls into one of two recovery classes. The classification is load-bearing for done-criterion #1 of §8: rebuilding the CP from empty state must restore the fleet's desired-state guarantees within one reconcile cycle, not just "approximately reach steady state eventually".

- **Soft state — recoverable from agent inputs on the next checkin cycle, or acceptable as a one-window operational regression:**
  - `token_replay` — bootstrap nonces with 24h TTL. Loss extends the replay window by up to one TTL. Bounded; no breach.
  - `pending_confirms` — in-flight activation deadlines. Loss could force the agent into an unnecessary local rollback when its confirm POST hits a 410. Mitigated by orphan-confirm recovery (#46): when the agent's reported `closure_hash` matches the verified target, the handler synthesises a confirmed row and returns 204 instead of 410.
  - `host_rollout_state` — per-host soak markers. Loss restarts soak windows from zero. Mitigated by agent-attested `last_confirmed_at` (#47): the agent persists the moment of its most recent successful confirm and echoes it on every checkin; the CP repopulates `last_healthy_since` from the attestation, clamped to `min(now, attested)`.
  - `host_reports` — SQLite-backed (#59 closed). Hydrated at boot via `boot: host_reports hydration complete`; outstanding `ComplianceFailure` / `RuntimeGateError` events survive CP restarts so the wave-promotion gate stays armed across the unlock window that motivated the original ring-buffer concern. Soft only because individual late-arriving reports retry on the next checkin.

- **Hard state — must come from signed artifacts pre-existing in git or from operator-declared trust roots:**
  - `cert_revocations` — agent-cert revocation list. Loss is a **security regression** — previously-revoked certs become valid again. Mitigated by the signed `revocations.json` sidecar (#48): operator commits revocations to the fleet repo, CI signs the artifact with the same `ciReleaseKey` that signs `fleet.resolved.json`, the CP fetches + verifies + replays on every reconcile tick. Recovery from empty is "one tick later, table populated from the signed artifact."
  - `trust.json` — the trust roots themselves. Sourced from the flake at build time; rebuildable as long as the flake survives. Issue #41 tracks the deferred TPM-bound issuance CA.

The principle is *"the CP holds nothing whose loss creates a security regression on rebuild, and nothing whose loss creates more than a one-window operational regression."* That is the strict reading of §8 done-criterion #1; #46 (orphan-confirm recovery), #47 (`last_confirmed_at` attestation), and #48 (signed revocations sidecar) are what make it true.

---

## 7. Non-goals

Stated explicitly because pressure to add them will come and each dilutes the core:

- **Not a general-purpose imperative runner.** No "run this script on all hosts". The only vocabulary is "target closure hash". If you need ad-hoc execution, you're outside the framework — use SSH.
- **Not a multi-tenant SaaS.** The control plane assumes a single administrative domain. Cross-org federation is out of scope.
- **Not a replacement for NixOS tooling.** `nixos-rebuild`, `nix flake`, `nix-store --verify` remain the ground truth. The framework orchestrates; it does not reimplement.
- **Not a cloud provisioning tool.** Fleet membership is declared; hosts are not auto-created from templates. If you want autoscaling, generate the flake from a higher-level tool and commit.
- **Not agentless.** Pull-based means an agent is required on every managed host. Acceptable cost for the sovereignty property.

For the operations-grade capabilities the open kernel intentionally does not ship — HA replication, real-time signed-state snapshots, SLA observability, audit packages, hosted CP, multi-tenant federation, fine-grained RBAC, long-running metrics warehousing — see [`docs/commercial-extensions.md`](docs/commercial-extensions.md). Those belong above the kernel, not inside it.

---

## 8. When is it actually done

Four falsifiable statements. If any is false, the design hasn't landed:

1. Destroying the control plane's database and rebuilding from empty state results in full fleet visibility within one reconcile cycle, with zero operator intervention beyond restarting the service. Strict reading: every CP-resident table either repopulates from agent inputs (soft state — `token_replay`, `pending_confirms`, `host_rollout_state`) or from a signed artifact in git (hard state — `cert_revocations` via the signed `revocations.json` sidecar, `trust.json` via the flake). See §6 Phase 10 for the per-table classification.
2. An auditor can be handed a host's hostname + a date range, and — without access to the control plane — produce a cryptographically-verifiable statement of "on this date, this host ran closure sha256-X, which was built from commit Y, and passed compliance controls Z₁..Zₙ with signed probe outputs matching the declared schemas".
3. The control plane's disk contents, stolen in their entirety, yield zero plaintext secret material.
4. A deliberately-corrupted closure pushed to attic (bypassing CI) is rejected by every agent; a deliberately-modified `fleet.resolved` served by the control plane is rejected by the control plane's own signature verification.

If all four hold, the slogan is true. If not, find the gap and close it before calling the framework done.

---

## 9. Source tree map

```
nixfleet/
├── flake.nix                      ← entry point, inputs, flake-parts wiring
├── Cargo.toml                     ← Rust workspace root
├── crane-workspace.nix            ← Nix wrapper around crane for Rust builds
│
├── ARCHITECTURE.md                ← this file
├── README.md, CHANGELOG.md, etc.  ← consumer-facing docs
├── DISASTER-RECOVERY.md           ← CP teardown procedure
├── SECURITY.md                    ← vuln disclosure policy
│
├── contracts/                     ← schemas. Top-level so import-tree skips
│   ├── host-spec.nix              │  them. They declare options; impls
│   ├── persistence.nix            │  satisfy them. NO mechanism here.
│   └── trust.nix                  ↓
│
├── impls/                         ← pluggable contract implementations,
│   ├── persistence/impermanence.nix
│   ├── keyslots/tpm/
│   ├── gitops/forgejo.nix
│   └── secrets/default.nix        ↑  exposed as flake.scopes.<family>.<impl>
│
├── lib/                           ← public API (mkHost, mkFleet, ...)
│   ├── default.nix                │  wired entry: imports flake inputs
│   ├── mk-fleet.nix               │  pure entry: just nixpkgs lib
│   ├── mk-host.nix                │
│   └── mk-vm-apps.nix             ↓
│
├── modules/                       ← flake-parts modules (auto-imported by
│   ├── flake-module.nix           │  import-tree, except _-prefixed files)
│   ├── apps.nix                   │  These declare flake outputs:
│   ├── formatter.nix              │    flake.lib, .scopes, .nixosModules
│   ├── options-doc.nix            │    perSystem.apps, .packages, .checks
│   ├── rust-packages.nix          │    .devShells, .formatter
│   │
│   ├── core/                      ← minimal NixOS/Darwin glue
│   │   ├── _nixos.nix             │  hostSpec → standard options,
│   │   └── _darwin.nix            ↓  flake-mode nix prereqs.
│   │
│   ├── scopes/nixfleet/           ← framework runtime services
│   │   ├── _agent.nix             │  systemd unit for the agent
│   │   ├── _agent-darwin.nix      │  launchd unit for the agent (macOS)
│   │   ├── _control-plane.nix     │  systemd unit for the CP
│   │   ├── _cache.nix             │  binary-cache client wiring
│   │   ├── _microvm-host.nix      │  microvm host (bridge, NAT, dnsmasq)
│   │   ├── _operator.nix          │  workstation tools (mint-token, etc.)
│   │   └── _trust-json.nix        ↓  shared helper: build trust.json
│   │
│   └── tests/                     ← flake-parts entries that register
│       ├── eval.nix               │  the checks that the test fabric runs
│       ├── harness.nix            │
│       ├── _agent-v2-trust.nix    │
│       ├── _cp-v2-trust.nix       │
│       └── _trust-options.nix     ↓
│
├── crates/                        ← the Rust workspace
│   ├── nixfleet-proto/            ← shared types (boundary contracts)
│   ├── nixfleet-canonicalize/     ← JCS canonicalizer (lib + bin)
│   ├── nixfleet-reconciler/       ← pure decision engine (lib only)
│   ├── nixfleet-agent/            ← per-host actuator daemon
│   ├── nixfleet-control-plane/    ← Axum HTTP server + reconcile loop
│   ├── nixfleet-cli/              ← operator workstation tools
│   ├── nixfleet-release/          ← CI release pipeline orchestrator
│   └── nixfleet-verify-artifact/  ← offline verifier for auditors
│
├── tests/                         ← test code, fixtures, harness
│   ├── fixtures/                  │  Static QEMU references
│   ├── harness/                   │  microvm.nix scenarios
│   └── lib/mk-fleet/              ↓  positive + negative eval fixtures
│
└── docs/                          ← human-readable docs
    ├── README.md, CONTRACTS.md, trust-root-flow.md, harness.md,
    ├── commercial-extensions.md
    ├── rfcs/                      │  RFC-0001 / 0002 / 0003
    └── adr/                       ↓  ADR 001–012, design decisions
```

Convention: `_*.nix` is **skipped by `import-tree`**. Files like `_agent.nix` are imported *explicitly* by `lib/mk-host.nix`. This is why agent/CP modules end up in every host's module list while test modules under `modules/tests/` only register via their non-prefixed siblings.

---

## 10. The Nix layer

### 10.1 Flake wiring

[`flake.nix`](flake.nix) is the entry point. Three jobs:

1. Declares **inputs** — `nixpkgs`, `darwin`, `home-manager`, `flake-parts`, `import-tree`, `disko`, `microvm`, `crane`, `lanzaboote`, `treefmt-nix`, `nixos-anywhere`, `nixos-hardware`, `impermanence`.
2. Picks the **system matrix** — `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin`.
3. Calls **`flake-parts.lib.mkFlake`** with `./modules/` auto-imported by `import-tree`.

```nix
outputs = inputs:
  inputs.flake-parts.lib.mkFlake { inherit inputs; } (
    (inputs.import-tree ./modules)
    // { systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ]; }
  );
```

`import-tree` walks `modules/`, skips `_*.nix`, returns an attrset of flake-parts modules; `mkFlake` merges them. This decomposition is why outputs (apps, packages, checks, devShells, lib, scopes) live in five small files (`flake-module.nix`, `apps.nix`, `formatter.nix`, `options-doc.nix`, `rust-packages.nix`) rather than one monolith.

`nixpkgs` is pinned to `nixos-unstable`; the framework re-pins consumers via `follows`, so a fleet's effective nixpkgs = the framework's. `impermanence` is required only by fleets that import `flake.scopes.persistence.impermanence`; inert otherwise.

### 10.2 Public API (`lib/`)

Four exports: `mkHost`, `mkFleet`, `mkVmApps`, plus `mergeFleets` and `withSignature`. Wiring in [`lib/default.nix`](lib/default.nix):

```nix
{ inputs, lib }: let
  mkFleetImpl = import ./mk-fleet.nix { inherit lib; };
in {
  mkHost     = import ./mk-host.nix    { inherit inputs lib; };
  mkVmApps   = import ./mk-vm-apps.nix { inherit inputs; };
  inherit (mkFleetImpl) mkFleet mergeFleets withSignature;
}
```

`mkFleet` is **pure** (just needs `lib`), so the canonicalize binary and eval-only tests can import `lib/mk-fleet.nix` directly without dragging in flake inputs. `mkHost` and `mkVmApps` need `inputs` because they build actual systems / spawn QEMU.

#### `mkHost` — the primary API ([`lib/mk-host.nix`](lib/mk-host.nix))

One function. Returns a NixOS or Darwin system, ready for `nixos-rebuild` / `darwin-rebuild`.

```nix
mkHost {
  hostName     = "my-server";          # required
  platform     = "x86_64-linux";       # selects nixosSystem vs darwinSystem
  stateVersion = "24.11";              # NixOS only
  hostSpec     = { userName = "deploy"; rootSshKeys = [ "ssh-ed25519 …" ]; };
  modules      = [ … ];                # consumer modules
  isVm         = false;                # if true, inject test fixtures
  extraInputs  = {};                   # consumer inputs to make visible
}
```

Internally:

1. Picks `nixpkgs.lib.nixosSystem` or `darwin.lib.darwinSystem` based on `platform`.
2. Auto-injects framework modules: `contracts/host-spec.nix`, `contracts/persistence.nix`, `modules/core/_nixos.nix` or `_darwin.nix`, all six `modules/scopes/nixfleet/_*.nix`. (Darwin gets only the agent-darwin and core-darwin modules.)
3. Sets `hostSpec` defaults (`mkDefault`-wrapped so consumer overrides win).
4. Forces `hostSpec.hostName = hostName` exactly (never overrideable).
5. Merges consumer's `modules` last.

Every framework service module is auto-injected but **disabled by default**. Zero cost unless the host opts in (`services.nixfleet-agent.enable = true;` etc.) — [ADR-005](docs/adr/005-scope-self-activation.md). Per [ADR-001](docs/adr/001-mkhost-over-mkfleet.md), the framework deliberately exposes one builder; no fleet/org/role taxonomy.

#### `mkFleet` — the fleet topology ([`lib/mk-fleet.nix`](lib/mk-fleet.nix))

Consumes a fleet description and produces `fleet.resolved` — the canonical projection that CI signs and the control plane consumes. Five major parts:

1. **`hosts`** — atomic units. Each declares system, configuration, tags, channel.
2. **`tags`** — flat, non-hierarchical groupings.
3. **`channels`** — release trains. Each pins `rolloutPolicy`, `freshnessWindow`, `signingIntervalMinutes`, `reconcileIntervalMinutes`, `compliance.frameworks`.
4. **`rolloutPolicies`** — named strategies. Each declares `waves` (selector + soakMinutes), a `healthGate`, an `onHealthFailure` action.
5. **`edges`** + **`disruptionBudgets`** — DAG ordering and concurrent-change limits.

**Selector algebra**: `tags`, `tagsAny`, `hosts`, `channel`, `all`, `not`, `and`. No wildcards; resolves at eval time.

`mkFleet` runs **invariant checks** — every host's channel exists, every channel's policy exists, edges form a DAG, `freshnessWindow ≥ 2 × signingIntervalMinutes`, every selector resolves to ≥1 host. Compliance failures in `enforce` mode block the build before signing. Output is `fleet.resolved` with `null` placeholders for `signedAt`, `ciCommit`, `closureHash` — filled by `nixfleet-release` at CI time.

`mergeFleets` strict-merges multiple fleet inputs (collisions throw); `withSignature` stamps `meta` after CI builds.

#### `mkVmApps` — local VM lifecycle ([`lib/mk-vm-apps.nix`](lib/mk-vm-apps.nix))

Returns five flake apps: `build-vm`, `start-vm`, `stop-vm`, `clean-vm`, `test-vm`. Linux-only. The 37-line composer is thin; platform abstraction lives in [`lib/vm-platform.nix`](lib/vm-platform.nix), shared bash in [`lib/vm-helpers.sh`](lib/vm-helpers.sh), per-app scripts in [`lib/vm-scripts/`](lib/vm-scripts). State under `~/.local/share/nixfleet/vms/`.

#### Flake-output modules (`modules/*.nix`)

- **`modules/flake-module.nix`** — exports `flake.lib`, `flake.nixosModules.nixfleet-core`, **`flake.scopes.<family>.<impl>`**.
- **`modules/apps.nix`** — declares perSystem apps. Most importantly, **`validate`** — the single test-suite entry (`nix run .#validate -- --all` runs format, eval, host builds, Rust tests, VM scenarios). Also exposes the agent / CP / cli / canonicalize / verify-artifact / release binaries.
- **`modules/formatter.nix`** — `nix fmt` via treefmt-nix (Alejandra + shfmt + deadnix).
- **`modules/options-doc.nix`** — generates the Markdown options reference.
- **`modules/rust-packages.nix`** — wires crane to build the workspace, exports docs-site, declares `devShells.default`.

### 10.3 Contracts

Pure schemas under [`contracts/`](contracts). They declare options; they implement nothing. Kept top-level (not under `modules/`) so `import-tree` doesn't treat them as flake-parts modules and leak `assertions` into flake-level scope. The cross-reference for *every* boundary-crossing artifact is [`docs/CONTRACTS.md`](docs/CONTRACTS.md).

#### `hostSpec` — universal identity ([`contracts/host-spec.nix`](contracts/host-spec.nix))

Every host has one. Identity (hostname, primary user, home dir), locale (timezone, locale, keyboard layout), access (root password file, root SSH keys), networking hints, secrets-backend hints, platform marker (`isDarwin`). The agent reads `hostSpec.userName`; persistence reads it for ownership; core reads `hostSpec.hostName` and stamps it into `networking.hostName`.

[ADR-002](docs/adr/002-flags-over-roles.md): hostSpec carries identity only; behaviour is via scope `enable` options. [ADR-006](docs/adr/006-hostspec-extension.md): fleets extend hostSpec with their own options via plain NixOS modules.

#### `persistence` — what survives reboots ([`contracts/persistence.nix`](contracts/persistence.nix))

```nix
options.nixfleet.persistence = {
  enable      = lib.mkEnableOption "system-level persistence";
  persistRoot = lib.mkOption { type = str; default = "/persist"; };
  directories = lib.mkOption { type = listOf (either str (attrsOf anything)); default = []; };
  files       = lib.mkOption { type = listOf (either str (attrsOf anything)); default = []; };
};
```

Baseline contributions (`/etc/nixos`, `/etc/NetworkManager/system-connections`, `/var/lib/systemd`, `/var/lib/nixos`, `/var/log`, `/etc/machine-id`) are added regardless of impl. Other modules contribute their own paths (agent → `/var/lib/nixfleet`, CP → `/var/lib/nixfleet-cp`, secrets → `/etc/ssh/ssh_host_ed25519_key`). The active impl reads the merged list.

#### `trust` — the four roots ([`contracts/trust.nix`](contracts/trust.nix))

The most security-critical contract:

```nix
options.nixfleet.trust = {
  ciReleaseKey = mkOption { type = ciReleaseKeySlotType; … };  # typed (algorithm + public)
  cacheKeys    = mkOption { type = listOf str; … };            # opaque, for nix's trusted-public-keys
  orgRootKey   = mkOption { type = keySlotType; … };           # bare-string ed25519 (pinned)
};
```

Three roots declared in the flake; the fourth root — the per-host SSH key — is intrinsic to each host (generated by stock OpenSSH on first boot). Each `KeySlot` has `current`, `previous`, `rejectBefore`. The `ciReleaseKey` slot is **typed** to support both `ed25519` and `ecdsa-p256` (TPMs commonly support P-256 but not ed25519). The `orgRootKey` is pinned to ed25519 — bootstrap-token signing only, never reaches the CP. `cacheKeys` is forwarded verbatim to `nix.settings.trusted-public-keys`. Serialised to JSON at build time (see `_trust-json.nix` below) and read at runtime.

### 10.4 Pluggable impls (`flake.scopes.*`)

[ADR-008](docs/adr/008-release-abstraction.md) and the kernel/opinion split: framework declares contracts and ships **one** impl per family. Sibling impls are alternatives. Registered in `modules/flake-module.nix`:

```nix
flake.scopes = {
  persistence.impermanence = ../impls/persistence/impermanence.nix;
  keyslots.tpm             = ../impls/keyslots/tpm;
  gitops.forgejo           = import ../impls/gitops/forgejo.nix;
  gitops.gitea             = import ../impls/gitops/forgejo.nix;  # API identical
  secrets                  = ../impls/secrets;
};
```

- **`persistence.impermanence`** ([`impls/persistence/impermanence.nix`](impls/persistence/impermanence.nix)) — btrfs-rootwipe-on-boot. initrd moves `@root` to `old_roots/<timestamp>`, creates fresh empty `@root`; upstream `impermanence` then bind-mounts paths from `/persist/...` back. Old snapshots pruned at default 30-day retention. Two impl-specific options: `rootDevice`, `oldRootsRetentionDays`.

- **`keyslots.tpm`** ([`impls/keyslots/tpm/`](impls/keyslots/tpm)) — first-boot TPM key generation, idempotent re-export after impermanence wipe. `tpm2_createprimary` + `tpm2_evictcontrol` to a persistent handle (default `0x81010001`); exports public key to `/var/lib/nixfleet-tpm-keyslot/`; installs a `tpm-sign` shell wrapper. Configurable: `handle`, `algorithm` (default `ecdsa-p256`), `exportPubkeyDir`, `signWrapperName`. Does **not** handle disk encryption.

- **`gitops.forgejo` / `.gitea`** ([`impls/gitops/forgejo.nix`](impls/gitops/forgejo.nix)) — pure data, a URL builder. Returns `{ artifactUrl; signatureUrl }` for a Forgejo or Gitea host. Wire into `services.nixfleet-control-plane.channelRefsSource`.

- **`secrets`** ([`impls/secrets/default.nix`](impls/secrets/default.nix)) — backend-agnostic identity-path manager. Declares where decryption identities live (`identityPaths.{hostKey, userKey, extra}`); ensures the SSH host key exists at first boot; adds those paths to the persistence contract; computes `resolvedIdentityPaths` (read-only introspection hook). Does NOT wrap agenix / sops / vault — your fleet wires those itself.

Consumer pattern:

```nix
# fleet-repo/flake.nix
nixosConfigurations.web-01 = nixfleet.lib.mkHost {
  hostName = "web-01";
  platform = "x86_64-linux";
  modules = [
    nixfleet.scopes.persistence.impermanence
    nixfleet.scopes.secrets
    nixfleet.scopes.keyslots.tpm
    ./hardware/web-01.nix
    ({ ... }: {
      services.nixfleet-agent = { enable = true; controlPlane.url = "https://cp.example.com:8080"; };
      hostSpec = { userName = "deploy"; rootSshKeys = [ "ssh-ed25519 …" ]; };
    })
  ];
};
```

### 10.5 Runtime service modules (`modules/scopes/nixfleet/`)

All underscore-prefixed (skipped by import-tree) and explicitly imported by `lib/mk-host.nix`. Each defaults to `enable = false`.

#### `_agent.nix` — Linux agent service

Key options: `enable`, `controlPlaneUrl`, `machineId`, `pollInterval` (60s default), `trustFile` (materialised from `nixfleet.trust`), `tls.{caCert, clientCert, clientKey}`, `bootstrapTokenFile`, `stateDir` (`/var/lib/nixfleet-agent`), `complianceGate.mode`, `package` (escape hatch for harness/vendor). Activation: materialises `trust.json` via `environment.etc`; installs `Type=simple, Restart=always, RestartSec=30, NoNewPrivileges=true`; contributes `/var/lib/nixfleet` to `nixfleet.persistence.directories`.

#### `_agent-darwin.nix` — macOS agent

Same schema plus `sshHostKeyFile` (default `/etc/ssh/ssh_host_ed25519_key`) and `tags` (passed via `NIXFLEET_TAGS` env). Differences: launchd instead of systemd (`KeepAlive`, `RunAtLoad`, `ThrottleInterval=10`); 15-second `sleep` in ExecStart to defend two boot races (NTP not synced → rustls cert "not yet valid"; agenix not yet decrypted → cert files missing); `launchctl kickstart -k` in postActivation forces clean restart even on unchanged plist; `environment.etc.<…>.text` instead of `.source` because Darwin's flake-source symlinks are unreliable.

#### `_control-plane.nix` — CP service

Richest module. Key options:

| Option | Default | Purpose |
|---|---|---|
| `listen` | `0.0.0.0:8080` | TLS bind |
| `tls.{cert, key, clientCa}` | required | mTLS server material |
| `artifactPath` / `signaturePath` | `/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json{,.sig}` | local signed artifact |
| `trustFile` | `/etc/nixfleet/cp/trust.json` | materialised from `nixfleet.trust` |
| `freshnessWindowMinutes` | `1440` (24h) | max accepted age of `meta.signedAt` |
| `confirmDeadlineSecs` | `360` | magic-rollback deadline |
| `fleetCaCert`, `fleetCaKey` | required for issuance | for `/v1/enroll` and `/v1/agent/renew` |
| `auditLogPath` | `/var/lib/nixfleet-cp/issuance.log` | append-only cert-issuance log |
| `dbPath` | `/var/lib/nixfleet-cp/state.db` | SQLite |
| `closureUpstream` | `null` | optional binary cache for `/v1/agent/closure/<hash>` |
| `rolloutsDir` | `null` | pre-signed rollout manifests on disk (primary) |
| `rolloutsSource.{artifactUrlTemplate, signatureUrlTemplate, tokenFile}` | `null` | on-demand HTTP fallback when `rolloutsDir` misses |
| `channelRefsSource.{artifactUrl, signatureUrl, tokenFile}` | `null` | upstream poll for `fleet.resolved` |
| `revocationsSource.{artifactUrl, signatureUrl, tokenFile}` | `null` | upstream poll for `revocations.json` sidecar |
| `strict` | `false` | refuse to start if `tls.clientCa` or `revocationsSource` is unset |
| `package` | self | escape hatch |

Long-running systemd service (`Type=simple`) with `ProtectSystem=strict`, `PrivateTmp=true`, etc. The CP does **not** use a systemd timer — it has its own internal 30-second reconcile loop. `systemd.tmpfiles.rules` auto-bootstraps `observed.json` to an empty skeleton on first deploy.

#### `_cache.nix` — binary-cache client

Trivial: declares `services.nixfleet-cache.{cacheUrl, publicKey}`; appends to `nix.settings.substituters` and `nix.settings.trusted-public-keys`. Format-agnostic.

#### `_microvm-host.nix` — microVM host wiring

Bridges, NAT, dnsmasq DHCP. Default bridge `nixfleet-br0`, `10.42.0.1/24`. The microVMs themselves are defined by your fleet via upstream `microvm.vms`.

#### `_operator.nix` — workstation tools

Adds `nixfleet-cli` (`nixfleet`, with subcommands `mint-token`, `derive-pubkey`, `mint-operator-cert`) to `environment.systemPackages`. Optional `orgRootKeyFile` exposed via `NIXFLEET_OPERATOR_ORG_ROOT_KEY`. **Crucially**: the org root *private* key is encrypted to the operator user only; the CP never decrypts it (it only verifies token signatures with the public half declared in `config.nixfleet.trust.orgRootKey.current`).

#### `_trust-json.nix` — shared trust serialiser

Helper imported by `_agent.nix`, `_control-plane.nix`, `_agent-darwin.nix`. Builds the JSON payload for `/etc/nixfleet/{agent,cp}/trust.json`. `schemaVersion = 1` is **required** per [`docs/trust-root-flow.md`](docs/trust-root-flow.md) §7.4 — binaries refuse to start on unknown versions.

#### Core glue (`modules/core/`)

`_nixos.nix`: flake-only `nix.nixPath`, `experimental-features`, hostName/timeZone/locale/keyMap/xkb from `hostSpec`, root SSH keys + hashed password file, imports `contracts/trust.nix`. `_darwin.nix` is even smaller — `system.stateVersion`, `system.primaryUser`, disables `verifyNixPath`, marks `hostSpec.isDarwin = true`. [ADR-009](docs/adr/009-core-hardening-audit.md) trimmed core down from a previous fat version.

---

## 11. The Rust layer

### 11.1 Crate map

Eight crates. Three boundary (types, canonicalisation, decision engine); five binaries. Dependency direction: **proto → canonicalize → reconciler → consumers**. No cross-deps among consumers.

```
                ┌─────────────────────────────────────────────┐
                │              nixfleet-proto                 │
                │  (boundary types: FleetResolved, wire,      │
                │   trust, revocations, rollout manifest)     │
                └────────────────────┬────────────────────────┘
                                     │
                  ┌──────────────────┼─────────────────┐
                  ▼                  ▼                 ▼
       ┌────────────────────┐   ┌────────────┐   ┌──────────────────┐
       │ nixfleet-          │   │ used by    │   │ used by          │
       │ canonicalize       │   │ everyone   │   │ everyone         │
       │ (JCS, RFC 8785)    │   └────────────┘   └──────────────────┘
       └─────────┬──────────┘
                 │
                 ▼
       ┌────────────────────┐
       │ nixfleet-          │
       │ reconciler         │
       │ (verify_artifact,  │
       │  reconcile fn,     │
       │  evidence verify)  │
       └─┬──────────────────┘
         │
   ┌─────┴──────┬──────────────┬──────────────┬──────────────┐
   ▼            ▼              ▼              ▼              ▼
┌──────┐   ┌────────┐   ┌──────────┐   ┌──────────┐   ┌────────────────┐
│agent │   │ control│   │ release  │   │  cli     │   │verify-artifact │
└──────┘   └────────┘   └──────────┘   └──────────┘   └────────────────┘
 per-host    Axum +       CI build      operator     offline auditor
 actuator    SQLite       pipeline      tools         tool
```

### 11.2 Boundary crates

#### `nixfleet-proto` — shared types

Canonical definitions for every artifact and message. Modules:

- **`fleet_resolved.rs`** — `FleetResolved`, `Host`, `Channel`, `RolloutPolicy`, `Wave`, `DisruptionBudget`, `Edge`, `Meta`, `Compliance`, `HealthGate`, `OnHealthFailure` enum.
- **`agent_wire.rs`** — `CheckinRequest/Response`, `EvaluatedTarget`, `ConfirmRequest`, `ReportRequest`, `ReportEvent`. Constant `PROTOCOL_MAJOR_VERSION = 1` (header `X-Nixfleet-Protocol`).
- **`enroll_wire.rs`** — `BootstrapToken`, `TokenClaims`, `EnrollRequest/Response`, `RenewRequest/Response`.
- **`revocations.rs`** — `Revocations`, `RevocationEntry`.
- **`rollout_manifest.rs`** — `RolloutManifest`, `HostWave`, `fleetResolvedHash` (anchor against mix-and-match).
- **`trust.rs`** — `TrustConfig`, `KeySlot`, `TrustedPubkey`.
- **`compliance.rs`** + **`evidence_signing.rs`** — typed signed payloads for every evidence event.

Conventions: optional fields use `Option<T>` with `#[serde(default)]` but **no** `skip_serializing_if` — `null` is *present*, important for JCS byte stability across Nix → Rust round-trips. **No** `#[serde(deny_unknown_fields)]` — contracts evolve additively. Object key sorting + deterministic number formatting is the canonicalize crate's job, not serde's.

#### `nixfleet-canonicalize` — JCS

Library + tiny binary. The library is one function:

```rust
pub fn canonicalize(input: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(input)?;
    serde_jcs::to_string(&value)
}
```

Every signer and every verifier feeds artifacts through this. Pinned `serde_jcs 0.2`, single source of truth. The binary is `cat`-style for use in CI sign hooks and tests.

#### `nixfleet-reconciler` — pure decision engine

The brain of the control plane, but as a pure library. No I/O, no state, no side effects. Two main exports:

```rust
pub fn verify_artifact(
    artifact_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &[&TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<FleetResolved, VerifyError>
```

Steps: parse → re-canonicalise (assert byte-for-byte match) → verify signature against each trusted key (ed25519 or ecdsa-p256, algorithm tag from `meta.signatureAlgorithm`) → freshness check (`now - meta.signedAt < freshness_window`) → `reject_before` check (compromise switch) → `schemaVersion == 1`. Returns parsed `FleetResolved` or detailed `VerifyError` (10 variants). Same path is used for `Revocations` and `RolloutManifest` via the `SignedSidecar` trait. Rollout manifests get an extra step: recompute `SHA-256(canonical(manifest))` and assert it equals the advertised `rolloutId` (content addressing).

```rust
pub fn reconcile(
    fleet: &FleetResolved,
    observed: &Observed,
    now: DateTime<Utc>,
) -> Vec<Action>
```

Inputs: verified fleet, `Observed` snapshot (channel refs, host states, active rollouts, compliance failures), current time. Output: a list of `Action`s (`OpenRollout`, `DispatchHost`, `PromoteWave`, `ConvergeRollout`, `HaltRollout`, `SoakHost`, `ChannelUnknown`, `Skip`, `WaveBlocked`).

Internal modules: `host_state.rs` (`HostRolloutState` lives in `nixfleet-proto`; reconciler + CP both consume), `rollout_state.rs` (`RolloutState` + `advance_rollout()`), `budgets.rs` (disruption budget enforcement — currently scaffolded), `edges.rs` (DAG ordering — reserved for future), `verify.rs` (`verify_artifact`, `verify_rollout_manifest`, `verify_revocations`, `SignedSidecar` trait, `compute_canonical_hash`, `compute_rollout_id`), `evidence.rs` (`verify_canonical_payload` for host-signed compliance evidence using OpenSSH ed25519 pubkeys), `manifest.rs` (`project_manifest`, `compute_rollout_id_for_channel`).

### 11.3 Runtime binaries

#### `nixfleet-agent` — per-host actuator

Long-running daemon. Flags set by the NixOS module: `--control-plane-url`, `--machine-id`, `--poll-interval`, `--trust-file`, `--ca-cert`, `--client-cert`, `--client-key`, `--bootstrap-token-file`, `--state-dir`, `--compliance-mode`.

Main loop: load trust → enrol if no cert + bootstrap token present → build mTLS client → `run_boot_recovery()` (handles ADR-011 self-switch convergence) → loop every `poll_interval`: POST `/v1/agent/checkin`; if response.target set, fetch + verify rollout manifest, pre-realise (`nix-store --realise <closure>` with cache_keys signature verify), activate (`systemd-run --unit=nixfleet-switch -- switch-to-configuration switch` on Linux, `setsid -c` on Darwin — both detached so they survive agent self-restart during NixOS reload, ADR-011), poll `/run/current-system` every 2s up to 300s, post-verify `basename == expected`, run compliance gate if enabled, POST `/v1/agent/confirm`, clear `last_dispatched`. On failure: POST `/v1/agent/report` with signed evidence. If cert TTL <50%: POST `/v1/agent/renew`.

Key modules: `comms.rs` (mTLS reqwest, 10s connect, 30s per-request), `activation.rs` (three-stage validation, fire-and-forget launch, lock coordination via `/run/nixos/switch-to-configuration.lock`, `ActivationOutcome` enum), `enrollment.rs` (CSR generation + enrol + 50% TTL renew), `checkin_state.rs` (`last_confirmed_at` + `last_dispatched`), `compliance.rs` (Pass / Failures / Skipped / GateError; `auto` mode → Permissive if collector present, Disabled if absent), `evidence_signer.rs` (loads `/etc/ssh/ssh_host_ed25519_key`, JCS-canonicalises, ed25519-signs, base64), `freshness.rs`, `manifest_cache.rs` (content-address verification), `recovery.rs` (`run_boot_recovery()`), `host_facts/` (Linux reads boot_id from `/proc/sys/kernel/random/boot_id`; Darwin uses hardware UUID).

What it never does: accept arbitrary commands (vocabulary is `target = sha256-X`); trust a CP-recommended closure without cache-key verification; hold long-lived credentials beyond 30-day mTLS cert + machine-lifetime SSH host key.

#### `nixfleet-control-plane` — Axum + SQLite + reconcile loop

Long-running HTTPS server. Two subcommands: `serve` and `tick` (one-shot, for tests).

Routes (under `/v1/` with protocol-version middleware):

```
GET  /healthz                          → { ok, version, last_tick_at }
GET  /v1/whoami                        → { cn, issuedAt }
POST /v1/enroll                        → 30-day cert from bootstrap token
POST /v1/agent/renew                   → re-issue cert from existing mTLS identity
POST /v1/agent/checkin                 → { target?, revocations? }
POST /v1/agent/confirm                 → marks host_dispatch_state row confirmed
POST /v1/agent/report                  → ingests telemetry events
GET  /v1/agent/closure/{hash}          → proxies to binary cache (optional)
GET  /v1/channels/{name}               → channel metadata
GET  /v1/hosts                         → { hostname: { online, current_generation } }
GET  /v1/rollouts/{rolloutId}          → manifest JSON (mTLS-gated)
GET  /v1/rollouts/{rolloutId}/sig      → manifest signature bytes
```

mTLS enforced at TLS handshake when `--client-ca` set. Agent routes authenticate solely via verified client cert (CN matches request hostname). No admin routes in the open kernel — fine-grained operator RBAC is intentionally out of scope (see [`docs/commercial-extensions.md`](docs/commercial-extensions.md)).

State:
- **In-memory** (`RwLock`): `host_checkins: HashMap<hostname, HostCheckinRecord>`, `channel_refs: HashMap<channel, git_ref>`, rollout manifest cache, `last_tick_at`.
- **SQLite** (`/var/lib/nixfleet-cp/state.db`, refinery-managed migrations):
  - `token_replay` (24h TTL) — soft state.
  - `cert_revocations` — **hard state**, replayed from signed `revocations.json` sidecar every reconcile tick.
  - `host_dispatch_state` (hostname PK, rollout_id, channel, wave, target_closure_hash, target_channel_ref, dispatched_at, confirm_deadline, confirmed_at, state ∈ {`pending`, `confirmed`, `rolled-back`, `cancelled`}) — operational, one row per host.
  - `dispatch_history` (id PK, hostname, rollout_id, channel, wave, target_closure_hash, target_channel_ref, dispatched_at, terminal_state ∈ {`converged`, `rolled-back`, `cancelled`}, terminal_at) — audit log; one row per dispatch event. Pre-#81 these two lived in a single `pending_confirms` table; the split landed in V006.
  - `host_rollout_state` (rollout_id, hostname, host_state, last_healthy_since, updated_at) — soak-window tracking, repopulated from agent-attested `last_confirmed_at` on rebuild.
  - `host_reports` (event_id, hostname, received_at, event_kind, rollout, signature_status, report_json) — telemetry.
- **Filesystem**: `artifact_path`, `signature_path`, `observed_path`.

Reconcile loop (every 30s) reads inputs, calls `verify_artifact()`, projects `Observed` from in-memory checkins + SQLite, calls `reconcile()`, processes the resulting `Vec<Action>` against SQLite (UPSERT `host_dispatch_state` + INSERT `dispatch_history` on dispatch, update `host_rollout_state`, etc.).

Background tasks: `reconcile_loop` (30s), `channel_refs_poll` (60s — full `verify_artifact` on fetched bytes, update in-memory map), `revocations_poll` (60s — same trust pipeline; replay into `cert_revocations` table on every tick), `rollback_check_loop` (10s — scan `state='pending' AND confirm_deadline < now`, mark `rolled-back`, stamp `dispatch_history`), `prune_timer` (delete old `token_replay`, archive old `host_reports`). All share a `tokio::sync::CancellationToken` plumbed from `main`; `signal::ctrl_c()` triggers `axum_server::Handle::graceful_shutdown` (25s drain) followed by cancellation fan-out; `drain_background_tasks` gathers JoinHandles with a 30s deadline.

**On-demand HTTP source — `rollouts_source`**: fetches a rollout manifest lazily when `GET /v1/rollouts/<rolloutId>` misses `--rollouts-dir`. URL templates with literal `{rolloutId}` token. **Trust posture**: the CP only checks `sha256(manifest) == rolloutId` (content-addressing). It does **not** verify the signature. The agent verifies the signature against `ciReleaseKey` on receipt. Even when forwarding a signed manifest, the CP never pretends to attest to it.

#### `nixfleet-cli` — operator workstation tools

An umbrella binary with operator subcommands. `nixfleet mint-token` reads the org root private key (32 raw bytes / hex / PEM PKCS#8), generates a nonce, builds `TokenClaims`, JCS-canonicalises, ed25519-signs, outputs the bootstrap-token JSON. `nixfleet derive-pubkey` reads a private key file and emits the base64 ed25519 pubkey — used once when bootstrapping the org root key. `nixfleet mint-operator-cert` mints a `clientAuth`-EKU X.509 cert signed by the offline fleet root for operator mTLS access.

There is no big "fleet management" CLI in the open kernel — operations happen through git commits and CI, not CLI commands.

#### `nixfleet-release` — CI release pipeline orchestrator

Most complex binary. Orchestrates **build → inject closureHash → stamp meta → canonicalise → sign → release**:

1. Enumerate hosts (`auto` = all; `auto:exclude=foo,bar`; or explicit list).
2. Build closures per host.
3. Per-closure push (optional `--push-cmd` hook; env: `NIXFLEET_HOST`, `NIXFLEET_PATH`, `NIXFLEET_CLOSURE_HASH`).
4. Evaluate `.#fleet.resolved`.
5. Inject `closureHash` per built host.
6. Stamp meta (`signedAt = now`, `ciCommit`, `signatureAlgorithm`).
7. Canonicalise via `nixfleet-canonicalize`.
8. Sign via `--sign-cmd` hook (env: `NIXFLEET_INPUT`, `NIXFLEET_OUTPUT`).
9. Smoke verify (re-parse, canonical round-trip, structural check).
10. Project per-channel rollout manifests (`rolloutId = SHA-256(canonical(manifest))`); sign each.
11. Atomic write of `releases/fleet.resolved.json{,.sig}`, `revocations.json{,.sig}`, `rollouts/<rolloutId>.json{,.sig}`.
12. Optional git ops (stage, commit, push).

The hook contract is what makes signing pluggable: framework doesn't care how you sign (TPM, HSM, YubiKey, KMS, software ed25519); it cares only that the hook reads canonical bytes from `$NIXFLEET_INPUT` and writes raw signature to `$NIXFLEET_OUTPUT`.

#### `nixfleet-verify-artifact` — offline auditor

Three subcommands (pure verification, no network): `artifact` (verify a `fleet.resolved`), `rollout-manifest` (verify a rollout manifest, asserts `rolloutId` hash matches), `probe` (verify a host-signed probe payload against an OpenSSH host pubkey). Given just signed artifacts plus trust roots, an auditor can verify the chain without ever touching the control plane.

---

## 12. Testing fabric

Three tiers, fastest-first.

### Tier C — eval-only (~5–15s, every PR)

- **`nix fmt -- --ci`** — Alejandra + shfmt + deadnix.
- **`nix flake check --no-build`** — eval every output across the system matrix.
- **`mkFleet-eval-tests`** — 14 fixtures (7 positive + 7 negative) under [`tests/lib/mk-fleet/`](tests/lib/mk-fleet). Positive fixtures must produce expected `.resolved.json` golden files; negative fixtures must throw expected eval errors.
- **`_agent-v2-trust.nix`, `_cp-v2-trust.nix`, `_trust-options.nix`** — eval-only assertions on agent/CP module wire shape (ExecStart flags, trust.json `schemaVersion = 1`, etc.).

### Tier B — Rust unit/integration (~15–30s, pre-push subset, full in CI)

- **`cargo nextest`** workspace-wide (currently 364 tests). Concentration: `nixfleet-control-plane` (Axum endpoint integration with in-process mTLS, SQLite transactions, mTLS CN matching, V001–V006 migration tests, graceful-shutdown drain), `nixfleet-reconciler` (state-machine transitions, signature round-trips, cycle detection), `nixfleet-proto` (round-trip serialisation, trust config), `nixfleet-canonicalize` (JCS golden vectors, RFC 8785 Appendix E), `nixfleet-release` (sign-smoke roundtrip + adversarial verify), `nixfleet-verify-artifact`, `nixfleet-agent` (boot-recovery convergence + per-variant DispatchHandler unit tests).
- **`cargo clippy`** with `-D warnings`.

### Tier A — microvm scenarios (minutes, nightly / on-demand)

Full integration via `runNixOSTest` hosting microvm.nix guests under one host VM (much faster than per-node QEMU). Linux x86_64 only (microvm.nix needs nested KVM). Scenarios under [`tests/harness/scenarios/`](tests/harness), registered in [`modules/tests/harness.nix`](modules/tests/harness.nix). Memory budget `max(4096, 3072 + N×256)`; fits fleet-50 in 16 GB.

| Scenario | Purpose |
|---|---|
| `fleet-harness-smoke` | 1 stub CP + 2 stub agents fetch fixture over mTLS within 60s |
| `fleet-harness-fleet-{2,5,10}` | Parameterised smoke for N agents |
| `fleet-harness-signed-roundtrip` | Real signed fixture → mTLS serve → agent verify-artifact accept |
| `fleet-harness-auditor-chain` | Offline `runCommand`: verify-artifact rejects bit-flips |
| `fleet-harness-corruption-rejection` | Bit-flip artifact + sig; assert typed `VerifyError` |
| `fleet-harness-manifest-tamper-rejection` | Same for rollout manifests; content-address mismatch |
| `fleet-harness-teardown` | **Real CP + real agents.** Wipe CP DB mid-run; assert state recovery within one reconcile cycle. The validation of done-criterion #1. |
| `fleet-harness-deadline-expiry` | Confirm-deadline timeout → 410 |
| `fleet-harness-stale-target` | Year-old fixture; agent's freshness gate rejects + posts `StaleTarget` |
| `fleet-harness-boot-recovery` | ADR-011: pre-staged stale `last_dispatched`; assert `check_boot_recovery` clears before poll loop |
| `fleet-harness-secret-hygiene` | Agent decrypts age secret; testScript greps CP disk + journal + audit; assert plaintext absent |
| `fleet-harness-rollback-policy` | Real CP + agent under `onHealthFailure = "rollback-and-halt"`; inject Failed via host-side sqlite3; assert RollbackSignal, agent rollback, Reverted, idempotency holds |
| `fleet-harness-concurrent-checkin` | Two agents in same tick window; assert no duplicate dispatch and ordered confirms |
| `fleet-harness-enroll-replay` | Bootstrap-token nonce replay rejected with 409 |
| `fleet-harness-future-dated-rejection` | Artifact with `meta.signedAt` past clock-skew slack rejected |
| `fleet-harness-module-rollouts-wire` | End-to-end manifest → checkin → confirm wiring under signed dispatch |

Real-binary harness nodes (`tests/harness/nodes/cp-real.nix` + `agent-real.nix`) consume `services.nixfleet-control-plane.enable = true` / `services.nixfleet-agent.enable = true` directly — the scenario surface is the operator surface. Stub nodes (`cp.nix`, `agent.nix`, `cp-signed.nix`, `agent-verify.nix`) keep their curl+jq scaffolding because they exercise routes the real CP doesn't expose (e.g. `GET /` for fleet-N substrate scaling, `GET /canonical.json{,.sig}` for the offline-auditor contract).

CI workflows: [`.github/workflows/ci.yml`](.github/workflows/ci.yml) — `format` job + `validate` job (`nix run .#validate`, default fast mode: format + flake eval + mkFleet-eval-tests + host builds for every `nixosConfiguration`). Pre-commit hook: format + real-SSH-key detector. Pre-push hook: format + `mkFleet-eval-tests` + `cargo nextest run --workspace`.

---

## 13. Glossary

| Term | Meaning |
|---|---|
| **Closure** | Nix's term for a store path plus all its transitive dependencies. The unit of deployment. Identified by hash. |
| **Closure hash** | `sha256` over the contents of a closure. Two identical closures share a hash. |
| **`fleet.resolved.json`** | Signed canonical projection of the fleet — hosts, channels, rolloutPolicies, waves, edges, budgets. CI-signed. |
| **Channel** | A release train (`stable`, `edge`). Each has its own rollout policy, freshness window, signing interval, compliance frameworks. |
| **Channel ref** | The git ref a channel is currently rolled out to. CI updates this when it produces a release. |
| **Rollout** | An in-flight transition of a channel from one ref to another. Has a state machine and per-host states. |
| **Wave** | A subset of a rollout's hosts dispatched together, with a shared soak window before the next wave proceeds. |
| **Rollout manifest** | Signed per-channel artifact freezing the rollout plan. Identified by content-address `rolloutId = sha256(canonical(manifest))`. |
| **Soak window** | Time a host must remain Healthy before being marked Soaked. Wave promotes only when all members are Soaked. |
| **Magic rollback** | If the agent doesn't post `/confirm` within `confirmDeadlineSecs`, the CP marks the dispatch rolled-back; the next checkin tells the agent to revert. |
| **Freshness window** | Per-channel max age of `meta.signedAt` accepted by `verify_artifact`. Defends against stale-target replay by a compromised CP. |
| **`rejectBefore`** | Compromise switch: any artifact with `meta.signedAt <` this timestamp is refused regardless of which key signed it. |
| **Trust roots** | The four signing keys: CI release key, cache keys, org root key, host SSH keys (see §4). |
| **mTLS** | Mutual TLS — both server and client present certificates. Agent identity is the cert's CN. |
| **Bootstrap token** | Org-root-signed claims (hostname, expectedPubkeyFingerprint, nonce, expiry) the agent uses *once* to enrol. |
| **JCS** | JSON Canonical Serialization (RFC 8785). Deterministic byte layout for signing. |
| **Persistence contract** | Schema declaring `directories`/`files` that survive reboots. Impls (e.g. impermanence) read this and apply their mechanism. |
| **`hostSpec`** | Universal identity carrier — hostname, primary user, locale, root SSH keys, etc. |
| **Scope** | A self-activating NixOS module (agent, CP, cache, microvm-host). Auto-included by `mkHost` but disabled by default. |
| **Contract impl** | A module that satisfies a contract. Lives under `impls/`, exposed as `flake.scopes.<family>.<impl>`. |
| **Stranger fleet test** | The discipline: a fleet you've never seen, with different operators and services, must be able to use the framework without any organisation-specific assumption. |
| **import-tree** | The flake input that auto-discovers and imports `.nix` files under `modules/`. Skips `_*.nix`. |
| **Underscore prefix** | `_*.nix` files are skipped by import-tree's auto-import. Imported explicitly by `mk-host.nix`. |

---

## 14. How to read this codebase

1. Start with [`flake.nix`](flake.nix) — five lines of meaningful logic. Open `lib/default.nix` next, then `lib/mk-host.nix`. That's the API surface.
2. Open `contracts/host-spec.nix`, `contracts/persistence.nix`, `contracts/trust.nix` — read each fully. Maybe 80 lines combined. They define the entire vocabulary.
3. Pick one runtime module (`modules/scopes/nixfleet/_agent.nix` is a good one) and read it with the corresponding crate's `src/main.rs` open in the other window. See how the NixOS module's `ExecStart` flags map to the crate's CLI.
4. Read `crates/nixfleet-proto/src/agent_wire.rs` and `crates/nixfleet-reconciler/src/verify.rs`. The boundary contracts and the verification logic. Most of the design pressure sits here.
5. RFCs come last: [RFC-0001](docs/rfcs/0001-fleet-nix.md) / [0002](docs/rfcs/0002-reconciler.md) / [0003](docs/rfcs/0003-protocol.md) in order.

Verification is cheap:

```sh
nix flake check --no-build                            # full eval, ~5s
nix run .#validate                                    # default fast mode
nix run .#validate -- --rust                          # add cargo nextest + clippy
nix run .#validate -- --vm                            # add microvm scenarios (Linux only)
nix build .#nixosConfigurations.<host>.config.system.build.toplevel   # one host's closure
```

---

## One-sentence summary

**Git is truth; CI is the notary; attic is the content store; the control plane is a router; agents are the last line of defense; and every boundary artifact carries its own proof.** Everything else is implementation.

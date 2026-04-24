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

The actual validation of the design principle. Destroy the control plane's SQLite. Restart it from empty state. Observe: it re-reads fleet.resolved from git, accepts agent check-ins, reconstructs fleet state within one reconcile tick per channel. No data lost.

Deliverable: a documented "disaster recovery" procedure that takes under 5 minutes from healthy-control-plane-gone to full-fleet-visibility restored.

---

## 7. Non-goals

Stated explicitly because pressure to add them will come and each dilutes the core:

- **Not a general-purpose imperative runner.** No "run this script on all hosts". The only vocabulary is "target closure hash". If you need ad-hoc execution, you're outside the framework — use SSH.
- **Not a multi-tenant SaaS.** The control plane assumes a single administrative domain. Cross-org federation is out of scope.
- **Not a replacement for NixOS tooling.** `nixos-rebuild`, `nix flake`, `nix-store --verify` remain the ground truth. The framework orchestrates; it does not reimplement.
- **Not a cloud provisioning tool.** Fleet membership is declared; hosts are not auto-created from templates. If you want autoscaling, generate the flake from a higher-level tool and commit.
- **Not agentless.** Pull-based means an agent is required on every managed host. Acceptable cost for the sovereignty property.

---

## 8. When is it actually done

Four falsifiable statements. If any is false, the design hasn't landed:

1. Destroying the control plane's database and rebuilding from empty state results in full fleet visibility within one reconcile cycle, with zero operator intervention beyond restarting the service.
2. An auditor can be handed a host's hostname + a date range, and — without access to the control plane — produce a cryptographically-verifiable statement of "on this date, this host ran closure sha256-X, which was built from commit Y, and passed compliance controls Z₁..Zₙ with signed probe outputs matching the declared schemas".
3. The control plane's disk contents, stolen in their entirety, yield zero plaintext secret material.
4. A deliberately-corrupted closure pushed to attic (bypassing CI) is rejected by every agent; a deliberately-modified `fleet.resolved` served by the control plane is rejected by the control plane's own signature verification.

If all four hold, the slogan is true. If not, find the gap and close it before calling the framework done.

---

## One-sentence summary

**Git is truth; CI is the notary; attic is the content store; the control plane is a router; agents are the last line of defense; and every boundary artifact carries its own proof.** Everything else is implementation.

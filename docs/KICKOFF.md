# v0.2 kickoff — cycle flow and stream prompts

This document describes how the v0.2 implementation cycle runs and hands each of the three parallel streams a self-contained starting prompt. Read this, `ARCHITECTURE.md`, and `docs/CONTRACTS.md` before taking a first commit.

---

## 1. Cycle flow

### Pre-kickoff (complete when this PR merges)

- `ARCHITECTURE.md`, RFCs 0001–0003, runnable spike, `docs/CONTRACTS.md`, `docs/KICKOFF.md` on `main`.
- Issues #1–#9, #12–#14 filed and labeled on `abstracts33d/nixfleet`.
- Twin issue #1 filed and labeled on `abstracts33d/nixfleet-compliance`.
- Tracking issue #10 on `abstracts33d/nixfleet` is the single board for the whole cycle.

### Phase 1 — Independent streams (weeks 1–N)

Three streams run without any cross-stream PRs:

- **Stream A — Infra:** work happens in the private `fleet` repo on `abstracts33d/fleet`.
- **Stream B — Nix:** work in `abstracts33d/nixfleet` and `abstracts33d/nixfleet-compliance`.
- **Stream C — Rust:** work in `abstracts33d/nixfleet`.

Each stream drives toward its first milestone (below). No stream modifies another stream's contract surface without a `contract-change` PR (see `docs/CONTRACTS.md` §VII).

**Cadence:**
- Each stream posts a dated, one-line status comment on its owning issue at least once every 3 working days.
- Per-milestone sync: a comment on tracking issue #10 when a stream hits its milestone, with a link to the PR(s) that closed it.

### Checkpoint 1 — First deliverables

Criteria:

| Stream | Milestone |
|---|---|
| A | `git push` to Forgejo triggers CI → closures land in attic → signed `fleet.resolved` artifact committed with CI key |
| B | `nix eval .#fleet.resolved --json` from a homelab-shaped flake produces a v1-valid artifact; `nixfleet-compliance` typed controls migrated with at least one `type = "both"` reference control per framework |
| C | `nixfleet-reconciler` pure-function crate with fixture-based tests covering every RFC-0002 state transition; `nixfleet-proto` crate with serde types mirroring `docs/CONTRACTS.md` §I; `nixfleet-agent` skeleton polls a stub CP over mTLS |

All three streams meet Checkpoint 1 before any cross-stream integration begins.

### Phase 2 — Cross-stream integration (Phase 4 convergence)

Once Checkpoint 1 is hit by all three:

1. Stream A's CI consumes Stream C's `nixfleet-canonicalize` tool to sign `fleet.resolved`.
2. Stream C's CP verifies signatures using `nixfleet.trust.*` pubkeys declared in Stream B's `fleet.nix`.
3. Stream C's agent verifies attic signatures on closure fetch against Stream A's attic key.
4. Stream B's typed compliance controls become consumable by Stream C's agent at runtime (Phase 7 of `ARCHITECTURE.md §6`).

This is where the **microvm.nix harness (#5)** earns its keep. Every integration failure lands as a harness scenario first, then fixes.

### Checkpoint 2 — Phase 4 convergence

Criteria:

- Magic rollback (#2) works end-to-end in the harness: a deliberate post-activation failure causes the host to revert within the confirm window.
- Compliance runtime gate (#4) blocks wave promotion on probe failure in the harness.
- Freshness window (#13) refuses a deliberately stale target in the harness.

After Checkpoint 2, the framework is operable. The remaining work is hardening and validation.

### Phase 3 — Validation

- Teardown test (#14) passes: destroy CP SQLite, restart, fleet reconstructs in one reconcile tick per channel. Validates done-criterion #1.
- Signing audit (#12): corrupted closure + modified `fleet.resolved` scenarios both rejected. Validates done-criterion #4.
- Zero-knowledge audit (#6 + #5): tcpdump shows no plaintext secrets. Validates done-criterion #3.
- Evidence chain audit (#4 + #12): produce a host/date → closure → commit → signed probe output chain. Validates done-criterion #2.

All four done-criteria hold → spine is complete.

### Phase 4 — Trim pass

Separate PR (not a stream). Removes the deprecated surface per the v0.1→v0.2 trimming document: strategy CLI flags, `mkHost` as user-facing API, operators module, ISO builder, darwin active support, RBAC, dynamic completions, live monitoring fields. Tagged as v0.2.0.

### Cycle summary

```
Pre-kickoff (this PR)
    │
    ▼
Checkpoint 0 — All three streams have access to contracts
    │
    ├─── Stream A ─── milestone ─┐
    ├─── Stream B ─── milestone ─┤
    └─── Stream C ─── milestone ─┤
                                 ▼
                          Checkpoint 1 — All three green
                                 │
                                 ▼
                    Phase 2 — Cross-stream integration
                                 │
                                 ▼
                    Checkpoint 2 — Phase 4 convergence
                                 │
                                 ▼
                    Phase 3 — Done-criteria validation
                                 │
                                 ▼
                    Phase 4 — Trim pass PR → v0.2.0 tag
```

### Merge discipline

- One issue = one PR. Squash-merge to `main`. PR title: `<type>: <short title> (#<issue>)`.
- Pre-commit (alejandra, real-SSH-keys guard) and pre-push (full test suite) hooks MUST pass.
- A PR that changes anything in `docs/CONTRACTS.md` requires a signoff from each stream that consumes the changed contract.
- Never force-push to `main`. Never merge your own PR without another stream's review on contract changes.
- Local branches keep the old names (`feat/`, `fix/`, `docs/`, `infra/`). Branch per issue; delete after merge.

---

## 2. Stream prompts

These are self-contained. A fresh session can pick one up cold.

---

### Stream A — Infra (M70q coordinator)

**Repo.** `abstracts33d/fleet` (private). NOT `abstracts33d/nixfleet`.

**Goal.** The M70q is the homelab fleet's trust-bearing coordinator. Forgejo hosts the git forge, attic hosts the binary cache, CI evaluates and signs, Caddy+Tailscale gate everything behind a private network.

**Reading list (before first commit):**
1. `abstracts33d/nixfleet/ARCHITECTURE.md` — §1.1–1.3 (flake / CI / attic), §3 (main flow), §6 Phase 0.
2. `abstracts33d/nixfleet/docs/CONTRACTS.md` — §II (trust roots) + §III (canonicalization).

**Milestone 1 deliverable.** A single NixOS host module (`hosts/m70q-attic.nix` in the `fleet` repo) that enables:

- Forgejo serving `git.home.arpa`, TLS via Caddy, Tailscale-only ACL on reverse proxy.
- Attic binary cache serving `cache.home.arpa`, signing closures with an ed25519 cache key.
- One of: Hercules CI agent OR Forgejo Actions self-hosted runner, capable of evaluating a flake and pushing closures to attic.
- Restic backing up `/var/lib/forgejo/` and the attic SQLite nightly.
- CI release ed25519 keypair in a TPM-backed keyslot (or HSM if available); public key published as a file Stream B can copy into `fleet.nix`.

**Acceptance.** A `git push` to Forgejo triggers CI → CI evaluates the flake → closures land in attic → CI signs a stub `fleet.resolved.json` with the CI release key → commit updates a channel pointer. End-to-end, on the M70q, no external dependencies.

**Cross-stream outputs** (hand off to Stream B after milestone):
1. CI release public key (ed25519, in OpenPGP-like armored form) → Stream B pins in `fleet.nix` as `nixfleet.trust.ciReleaseKey`.
2. Attic cache public key → Stream B pins as `nixfleet.trust.atticCacheKey`.
3. CP endpoint URL format → Stream C consumes.

**Non-goals for this stream.**
- Do not modify `abstracts33d/nixfleet`.
- Do not design the wire protocol or reconciliation logic.
- Do not implement compliance controls.

**First issue to pick up.** There is no nixfleet-repo issue for this stream — it happens in the `fleet` repo. Create a tracking issue in the `fleet` repo for milestone 1 and reference it from `abstracts33d/nixfleet#10`.

**Owned contracts from CONTRACTS.md.** §II #1 (CI release key), §II #2 (attic key). Stream A holds the private keys; Stream B declares the public halves.

---

### Stream B — Nix (schema + compliance)

**Repos.** `abstracts33d/nixfleet` + `abstracts33d/nixfleet-compliance`.

**Goal.** Produce `fleet.resolved.json` from a declared `fleet.nix`; migrate compliance controls to the typed `static` | `runtime` | `both` model; keep the Nix side of every boundary contract byte-compatible with Stream C's Rust consumers.

**Reading list (before first commit):**
1. `abstracts33d/nixfleet/ARCHITECTURE.md` — all.
2. `abstracts33d/nixfleet/rfcs/0001-fleet-nix.md` — all.
3. `abstracts33d/nixfleet/docs/CONTRACTS.md` — §I #1, #3, #5, §II (declarations), §III (canonicalization), §VII (amendment).
4. `abstracts33d/nixfleet/spike/` — the running prototype; `lib/mkFleet.nix` promotes to production.

**Milestone 1 deliverables.**
1. Promote `spike/lib/mkFleet.nix` → `lib/mkFleet.nix` in production shape. All RFC-0001 §4.2 invariants implemented and fail fast; new invariant: `channel.freshnessWindow ≥ 2 × signingIntervalMinutes` (gap captured in #13).
2. `nixfleet.trust.*` option tree in `modules/trust.nix`, with docstrings referencing CONTRACTS.md §II.
3. `abstracts33d/nixfleet-compliance#1` resolved: typed control migration, schema-versioned probe descriptors, JCS canonicalization contract declared, negative-test fixture per control. At least one `type = "both"` reference control per framework.
4. Baseline compliance control explicitly exempting the agent's outbound network path from any firewall-lock control — documented as a required baseline so that compliance landing does not cut agents off (CONTRACTS.md §I captures this in the probe registry; implement here).

**Acceptance.** `nix eval --json .#fleet.resolved` from the homelab example in `spike/examples/homelab/` produces a `schemaVersion: 1` artifact that serialized to JCS is byte-identical to what Stream C's canonicalizer produces on the same input.

**Cross-stream outputs.**
- Valid `fleet.resolved.json` emitted per CONTRACTS.md §I #1 — consumed by Stream C.
- Typed probe descriptors per CONTRACTS.md §I #3 — consumed by Stream C agent.
- `nixfleet.trust.*` options exposed — Stream A hands pubkeys, Stream B pins.

**Non-goals for this stream.**
- Do not write Rust code.
- Do not wire CI pipelines (Stream A).
- Do not design CP storage or wire protocol (Stream C).
- Do not touch `crates/*` beyond reading types to confirm mirror.

**First issues to pick up.** `abstracts33d/nixfleet#1`, then `abstracts33d/nixfleet-compliance#1`, then `abstracts33d/nixfleet#7`. Issue `#12` has a Nix portion (option tree) that lands with `#1`; the signing tooling lands in Stream C.

**Owned contracts from CONTRACTS.md.** §I #1 (producer), §I #3 (producer), §I #5 (producer), §II declarations, part of §III (canonicalization tooling specification).

---

### Stream C — Rust (reconciler + agent + CP + wire protocol)

**Repo.** `abstracts33d/nixfleet` — all work in `crates/`.

**Goal.** Promote the reconciler spike to production; build the agent skeleton, the control plane, the wire protocol crate, and the JCS canonicalization tool. Keep every contract byte-compatible with Stream B's Nix output.

**Reading list (before first commit):**
1. `abstracts33d/nixfleet/ARCHITECTURE.md` — all.
2. `abstracts33d/nixfleet/rfcs/0002-reconciler.md` — all.
3. `abstracts33d/nixfleet/rfcs/0003-protocol.md` — all.
4. `abstracts33d/nixfleet/docs/CONTRACTS.md` — §I #1, #2, #4, #6, §II (verification), §III (canonicalization), §IV (storage purity), §V (versioning), §VII.
5. `abstracts33d/nixfleet/spike/reconciler/` — ~200 lines; promotes to production.

**Milestone 1 deliverables.**
1. `crates/nixfleet-proto` — serde types for every artifact in CONTRACTS.md §I. `schemaVersion` roundtrip tests against Nix-generated fixtures. Decide and document: `deny_unknown_fields` vs ignore posture per artifact.
2. `crates/nixfleet-reconciler` — pure function `(Fleet, Observed, now) → Vec<Action>` from the spike, plus:
   - Signature verification step (RFC-0002 §4 step 0).
   - Freshness check against `channel.freshnessWindow`.
   - Fixture harness: every state machine transition from RFC-0002 §3 covered; regression test runner.
3. `crates/nixfleet-agent` — poll-only skeleton. Enrolls via bootstrap token, fetches mTLS cert, checks in on cadence, reports `currentGeneration`. No activation yet — logs the target it *would* activate.
4. `crates/nixfleet-control-plane` — Axum + SQLite + mTLS skeleton. Four endpoints per RFC-0003 §4. Every CP SQLite column carries a `-- derivable from:` line comment (CONTRACTS.md §IV rule).
5. `crates/nixfleet-cli` — minimum operator surface: `status`, `rollout trace <id>`.
6. `bin/nixfleet-canonicalize` — thin wrapper around the chosen JCS library; shell-invocable by Stream A's CI. Golden-file test per CONTRACTS.md §III. **Pin the JCS library choice in this PR.**

**Acceptance.**
- `cargo test` green across all crates.
- Roundtrip test: fixture `fleet.resolved.json` → serde → canonicalize → verify → identical to Nix-produced canonical bytes.
- Pure-function reconciler: every RFC-0002 §3 transition exercised from fixtures alone, no network, no filesystem beyond test inputs.
- CP storage audit table in `docs/CP-STORAGE.md`: column → derivation source (git / check-in / accepted loss).

**Cross-stream outputs.**
- `bin/nixfleet-canonicalize` consumed by Stream A's CI.
- `crates/nixfleet-proto` types are the ground truth that Stream B mirrors.

**Non-goals for this stream.**
- Do not write NixOS modules (Stream B).
- Do not implement compliance control logic or framework manifests (Stream B).
- Do not provision infrastructure (Stream A).
- Do not implement activation (`nixos-rebuild switch`) in milestone 1 — that's Phase 4 of the architecture, gated on Checkpoint 2.

**First issues to pick up.** `abstracts33d/nixfleet#2` (agent skeleton prep), `#3` (channel→rev reconciler wiring), `#12` Rust portion (signature verification code + canonicalize tool), `#13` (freshness enforcement). Full `#4`, `#9`, `#14` skeletons land here but complete in later checkpoints.

**Owned contracts from CONTRACTS.md.** §I #2 (producer and consumer), §I #4 (consumer), §I #6 (producer), §II verification logic, §III (canonicalization library pin + golden test), §IV (storage purity).

---

## 3. Rules of engagement

These apply equally to all three streams.

- **Contracts are law.** If a change touches `docs/CONTRACTS.md`, it is a cross-stream PR with signoff from every affected stream. No exceptions.
- **Evidence before claims.** "The agent works" means `cargo test` output + a harness scenario that exercises the claim. "The fleet evaluates" means `nix eval --json` + the output matches a pinned fixture.
- **Parallelize within a stream whenever safe.** The three streams are parallel; within each stream, if two tasks don't touch the same files, run them in parallel too.
- **Ask before destructive actions.** Deleting a column, removing a field, changing a signing key — none of those happen without a contract-change PR and at least one sync comment on #10.
- **Tracking issue #10 is the board.** Every milestone hit, every blocker, every cross-stream handoff gets a comment on #10. The issue is the running log of the cycle.
- **Don't merge your own PR.** Cross-stream PRs need the other stream's signoff. Intra-stream PRs need at minimum one self-review pass and the full test suite green.

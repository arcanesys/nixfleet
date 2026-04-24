# JCS Canonicalize Pin — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL — use `superpowers:executing-plans` or `superpowers:subagent-driven-development` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal.** Pin the RFC 8785 JCS library for the whole v0.2 fleet, ship `crates/nixfleet-canonicalize` (lib + thin stdin/stdout binary), prove byte-exact determinism with a golden-file test, and amend `docs/CONTRACTS.md §III` to lock the pin.

**Architecture.** New Rust crate at `crates/nixfleet-canonicalize/`. Lib exports `pub fn canonicalize(input: &str) -> anyhow::Result<String>` delegating to `serde_jcs::to_string(&serde_json::Value)`. Thin binary reads stdin, canonicalizes, writes stdout. No `nixfleet-proto` crate (ships later with the reconciler).

**Tech stack.** Rust edition 2021, `serde_jcs = "0.2"`, `serde_json = "1"`, `anyhow = "1"`. Integration tests at `tests/*.rs`. No workspace-level changes.

**Repo.** `abstracts33d/nixfleet` (origin). Worktree: `.worktrees/stream-c`. Branch after pre-flight: `feat/12-canonicalize-jcs-pin`.

**Execution convention.** Heavy commands (anything running `cargo test --workspace`, `cargo nextest run --workspace`, `nix build`, `nix flake check`, or `nix develop`) are marked **[USER RUNS]** — the implementing agent MUST NOT run them and should wait for the user's result. Cheap per-crate commands (`cargo build -p <crate>`, `cargo test -p <crate>`, `cargo metadata -p <crate>`) may be run by the agent.

---

## File Structure

**Create.**
- `crates/nixfleet-canonicalize/Cargo.toml`
- `crates/nixfleet-canonicalize/src/lib.rs`
- `crates/nixfleet-canonicalize/src/main.rs`
- `crates/nixfleet-canonicalize/tests/jcs_golden.rs`
- `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.json`
- `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical` (61 bytes, no trailing newline)

**Edit.**
- `docs/CONTRACTS.md` (§III only)
- `TODO.md` (one-line append)
- `Cargo.lock` (auto-regenerated; do not hand-edit)

**Must not touch.** `lib/`, `modules/`, `spike/`, `crates/{agent,cli,control-plane,shared}`.

---

## Task 0 — Pre-flight: rename the branch

Local-only rename; no remote exists yet.

- [ ] **Step 1.** Confirm current branch.

  Run: `git -C /home/s33d/dev/arcanesys/nixfleet/.worktrees/stream-c branch --show-current`
  Expected: `feat/12-canonicalize-proto`

- [ ] **Step 2.** Rename.

  Run: `git -C /home/s33d/dev/arcanesys/nixfleet/.worktrees/stream-c branch -m feat/12-canonicalize-proto feat/12-canonicalize-jcs-pin`

- [ ] **Step 3.** Verify.

  Run: `git -C /home/s33d/dev/arcanesys/nixfleet/.worktrees/stream-c branch --show-current`
  Expected: `feat/12-canonicalize-jcs-pin`

No commit — ref rename only, nothing staged.

---

## Task 1 — Cargo.toml + lib stub + main placeholder

A stub `src/lib.rs` (module-level doc only) lets Task 2's test file compile against an existing lib target; the test then fails with `cannot find function` instead of "file not found". A placeholder `src/main.rs` is needed because `Cargo.toml` declares `[[bin]] path = "src/main.rs"` — without the file, `cargo check -p nixfleet-canonicalize` and the workspace-level `cargo test --workspace` fail with "can't find bin at path" before reaching any test logic. Task 7 overwrites the placeholder with the real stdin/stdout wrapper.

**Files.**
- Create `crates/nixfleet-canonicalize/Cargo.toml`
- Create `crates/nixfleet-canonicalize/src/lib.rs` (stub)
- Create `crates/nixfleet-canonicalize/src/main.rs` (placeholder; overwritten in Task 7)

- [ ] **Step 1.** Write the manifest.

  File: `crates/nixfleet-canonicalize/Cargo.toml`
  ```toml
  [package]
  name = "nixfleet-canonicalize"
  version = "0.2.0"
  edition = "2021"
  description = "JCS (RFC 8785) canonicalizer for NixFleet signed artifacts"
  license = "MIT"
  repository = "https://github.com/arcanesys/nixfleet"
  homepage = "https://github.com/arcanesys/nixfleet"
  authors = ["nixfleet contributors"]

  [lib]
  name = "nixfleet_canonicalize"
  path = "src/lib.rs"

  [[bin]]
  name = "nixfleet-canonicalize"
  path = "src/main.rs"

  [dependencies]
  serde_jcs = "0.2"
  serde_json = "1"
  anyhow = "1"
  ```

- [ ] **Step 2.** Write an empty lib stub so the crate compiles before any function exists.

  File: `crates/nixfleet-canonicalize/src/lib.rs`
  ```rust
  //! JCS canonicalization library backing the `nixfleet-canonicalize`
  //! binary. Implementation follows in the next task.
  ```

- [ ] **Step 3.** Write the placeholder binary so the `[[bin]]` target builds. Task 7 overwrites this.

  File: `crates/nixfleet-canonicalize/src/main.rs`
  ```rust
  //! Placeholder. The stdin/stdout wrapper lands in a later task.
  //! This file exists so `[[bin]] path = "src/main.rs"` resolves and
  //! workspace-level `cargo test --workspace` does not fail with
  //! "can't find bin" while the real binary is still being built up.

  fn main() {
      unimplemented!("nixfleet-canonicalize: binary wrapper lands in a later task")
  }
  ```

- [ ] **Step 4.** Verify cargo picks up the crate and resolves `serde_jcs 0.2`.

  Run: `cargo metadata --format-version 1 --manifest-path crates/nixfleet-canonicalize/Cargo.toml 2>&1 | head -c 300`
  Expected: JSON beginning with `{"packages":[...` mentioning `nixfleet-canonicalize`. No error. `Cargo.lock` is updated with `serde_jcs` + transitive deps.

- [ ] **Step 5.** Verify the full crate (lib + placeholder bin) compiles.

  Run: `cargo check -p nixfleet-canonicalize 2>&1 | tail -5`
  Expected: `Finished` (no errors).

- [ ] **Step 6.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/Cargo.toml crates/nixfleet-canonicalize/src/lib.rs crates/nixfleet-canonicalize/src/main.rs Cargo.lock
  git commit -m "feat(canonicalize): scaffold crate with pinned serde_jcs 0.2"
  ```

---

## Task 2 — RED: golden-file test + fixtures

Test must fail to compile with `cannot find function \`canonicalize\``.

**Files.**
- Create `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.json`
- Create `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical`
- Create `crates/nixfleet-canonicalize/tests/jcs_golden.rs`

- [ ] **Step 1.** Write input fixture.

  File: `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.json`
  ```
  {"b":2,"a":{"z":null,"y":true,"x":[3,1,2]},"schemaVersion":1}
  ```

- [ ] **Step 2.** Write expected canonical fixture. **Exactly 61 bytes, no trailing newline.**

  File: `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical`

  Content (single line, no trailing newline):
  ```
  {"a":{"x":[3,1,2],"y":true,"z":null},"b":2,"schemaVersion":1}
  ```

- [ ] **Step 3.** Verify byte count.

  Run: `wc -c crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical`
  Expected: `61 crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical`

  If the count is 62, an editor or write tool appended a newline. Strip it:
  ```bash
  truncate -s 61 crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical
  wc -c crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical
  ```
  Re-verify the output is exactly `61`.

- [ ] **Step 4.** Write the failing golden test.

  File: `crates/nixfleet-canonicalize/tests/jcs_golden.rs`
  ```rust
  //! Golden-file JCS test (`docs/CONTRACTS.md §III`).
  //!
  //! Asserts the canonicalizer produces byte-exact output matching
  //! the committed golden. Runs on every push via pre-push
  //! `cargo nextest run --workspace`. Any drift = signature contract
  //! broken.

  use nixfleet_canonicalize::canonicalize;

  const GOLDEN_INPUT: &str = include_str!("fixtures/jcs-golden.json");
  const GOLDEN_CANONICAL: &str = include_str!("fixtures/jcs-golden.canonical");

  #[test]
  fn jcs_golden_bytes_match() {
      let produced = canonicalize(GOLDEN_INPUT).expect("canonicalize golden input");
      assert_eq!(
          produced, GOLDEN_CANONICAL,
          "JCS output drifted from golden — signature contract broken"
      );
  }
  ```

- [ ] **Step 5.** Verify the test FAILS to compile (RED).

  Run: `cargo build -p nixfleet-canonicalize --tests 2>&1 | tail -10`
  Expected: error message referring to `cannot find function \`canonicalize\`` or `unresolved import \`nixfleet_canonicalize::canonicalize\``. NO green build.

- [ ] **Step 6.** Commit the failing test (tests-first discipline).

  ```bash
  git add crates/nixfleet-canonicalize/tests
  git commit -m "test(canonicalize): add failing golden-file test and fixtures"
  ```

---

## Task 3 — GREEN: implement canonicalize()

**Files.** Modify `crates/nixfleet-canonicalize/src/lib.rs`.

- [ ] **Step 1.** Replace the stub with the minimal implementation.

  File: `crates/nixfleet-canonicalize/src/lib.rs` (full replacement)
  ```rust
  //! JCS canonicalization library backing the `nixfleet-canonicalize`
  //! binary. Pinned to `serde_jcs` per `docs/CONTRACTS.md §III`.
  //!
  //! Every signer and verifier in the fleet goes through this one
  //! function — do not reimplement in Nix, shell, or ad-hoc Rust.

  use anyhow::{Context, Result};

  /// Canonicalize an arbitrary JSON string to JCS (RFC 8785) form.
  ///
  /// Errors on malformed JSON. The returned string is the exact byte
  /// sequence every signer must feed to its signature primitive and
  /// every verifier must reconstruct before verification.
  pub fn canonicalize(input: &str) -> Result<String> {
      let value: serde_json::Value =
          serde_json::from_str(input).context("input is not valid JSON")?;
      serde_jcs::to_string(&value).context("JCS canonicalization failed")
  }
  ```

- [ ] **Step 2.** Verify the golden test passes.

  Run: `cargo test -p nixfleet-canonicalize --test jcs_golden 2>&1 | tail -15`
  Expected: `test jcs_golden_bytes_match ... ok` and `test result: ok. 1 passed; 0 failed`.

  If the assertion fails (hand-computed canonical doesn't match `serde_jcs` output), inspect:
  ```bash
  cargo test -p nixfleet-canonicalize --test jcs_golden -- --nocapture 2>&1 | sed -n '/JCS output drifted/,/^$/p'
  ```
  Then either fix the fixture OR file an upstream bug on `serde_jcs` — DO NOT change the lib to paper over.

- [ ] **Step 3.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/src/lib.rs
  git commit -m "feat(canonicalize): implement canonicalize() over serde_jcs"
  ```

---

## Task 4 — Idempotence test

Canonical form must be a fixed point: `canonicalize(canonicalize(x)) == canonicalize(x)`.

**Files.** Modify `crates/nixfleet-canonicalize/tests/jcs_golden.rs`.

- [ ] **Step 1.** Append the test.

  Append to the end of `tests/jcs_golden.rs`:
  ```rust

  #[test]
  fn canonicalize_is_idempotent() {
      let once = canonicalize(GOLDEN_INPUT).expect("canonicalize once");
      let twice = canonicalize(&once).expect("canonicalize canonical form");
      assert_eq!(once, twice, "canonical form must be a fixed point");
  }
  ```

- [ ] **Step 2.** Verify — should pass immediately (the property already holds).

  Run: `cargo test -p nixfleet-canonicalize --test jcs_golden 2>&1 | tail -10`
  Expected: `test result: ok. 2 passed; 0 failed`.

  If it fails, there is a bug in `serde_jcs`' re-canonicalization — file upstream; do not suppress.

- [ ] **Step 3.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/tests/jcs_golden.rs
  git commit -m "test(canonicalize): lock in idempotence of canonical form"
  ```

---

## Task 5 — Reorder-invariance test

Two inputs differing only in key order must canonicalize identically.

**Files.** Modify `crates/nixfleet-canonicalize/tests/jcs_golden.rs`.

- [ ] **Step 1.** Append the test.

  ```rust

  #[test]
  fn reordering_input_does_not_change_canonical_output() {
      let reordered = r#"{"schemaVersion":1,"a":{"x":[3,1,2],"z":null,"y":true},"b":2}"#;
      let original = canonicalize(GOLDEN_INPUT).expect("canonicalize original");
      let shuffled = canonicalize(reordered).expect("canonicalize shuffled");
      assert_eq!(
          original, shuffled,
          "canonical output must be invariant under input key ordering"
      );
  }
  ```

- [ ] **Step 2.** Verify.

  Run: `cargo test -p nixfleet-canonicalize --test jcs_golden 2>&1 | tail -10`
  Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 3.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/tests/jcs_golden.rs
  git commit -m "test(canonicalize): assert key-order invariance"
  ```

---

## Task 6 — Invalid-JSON rejection test

**Files.** Modify `crates/nixfleet-canonicalize/tests/jcs_golden.rs`.

- [ ] **Step 1.** Append the test.

  ```rust

  #[test]
  fn invalid_json_is_rejected() {
      let result = canonicalize("{not json");
      assert!(result.is_err(), "invalid JSON must be rejected");
  }
  ```

- [ ] **Step 2.** Verify.

  Run: `cargo test -p nixfleet-canonicalize --test jcs_golden 2>&1 | tail -10`
  Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 3.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/tests/jcs_golden.rs
  git commit -m "test(canonicalize): reject invalid JSON input"
  ```

---

## Task 7 — REFACTOR: binary stdin/stdout wrapper

**Files.** Overwrite the placeholder `crates/nixfleet-canonicalize/src/main.rs` (created in Task 1) with the real implementation.

- [ ] **Step 1.** Replace the placeholder with the real wrapper.

  File: `crates/nixfleet-canonicalize/src/main.rs` (full replacement)
  ```rust
  //! `nixfleet-canonicalize` — stdin JSON → JCS canonical stdout.
  //!
  //! Shell-invocable canonicalizer for Stream A's CI signing
  //! pipeline. Exit codes:
  //! - 0 — canonical bytes written to stdout
  //! - 1 — input was not valid JSON or canonicalization failed
  //! - 2 — I/O error reading stdin or writing stdout

  use std::io::{self, Read, Write};
  use std::process::ExitCode;

  fn main() -> ExitCode {
      let mut input = String::new();
      if let Err(err) = io::stdin().read_to_string(&mut input) {
          eprintln!("nixfleet-canonicalize: read stdin: {err}");
          return ExitCode::from(2);
      }

      let canonical = match nixfleet_canonicalize::canonicalize(&input) {
          Ok(s) => s,
          Err(err) => {
              eprintln!("nixfleet-canonicalize: {err:#}");
              return ExitCode::from(1);
          }
      };

      let mut stdout = io::stdout().lock();
      if let Err(err) = stdout.write_all(canonical.as_bytes()) {
          eprintln!("nixfleet-canonicalize: write stdout: {err}");
          return ExitCode::from(2);
      }

      ExitCode::SUCCESS
  }
  ```

- [ ] **Step 2.** Verify the binary builds.

  Run: `cargo build -p nixfleet-canonicalize --bin nixfleet-canonicalize 2>&1 | tail -5`
  Expected: `Finished` (no errors).

- [ ] **Step 3.** Shell round-trip smoke.

  Run: `echo '{"b":1,"a":2}' | cargo run -q -p nixfleet-canonicalize`
  Expected stdout (no trailing newline):
  ```
  {"a":2,"b":1}
  ```

  Verify exit code:
  Run: `echo '{"b":1,"a":2}' | cargo run -q -p nixfleet-canonicalize; echo "exit=$?"`
  Expected: `{"a":2,"b":1}exit=0` (output and exit=0 on the same or next line depending on trailing-newline rendering).

- [ ] **Step 4.** Commit.

  ```bash
  git add crates/nixfleet-canonicalize/src/main.rs
  git commit -m "feat(canonicalize): add stdin/stdout binary wrapper"
  ```

---

## Task 8 — Contract amendment (`docs/CONTRACTS.md §III`)

Load-bearing contract change. Per §VII any further pin move needs signoff from every stream that signs/verifies.

**Files.** Modify `docs/CONTRACTS.md`.

- [ ] **Step 1.** Replace the §III bullet block.

  Find exactly this block inside §III:
  ```markdown
  - **Library choice.** TBD — Stream C's first commit must pin one (`serde_jcs` or equivalent) and document it here. Requirements: RFC 8785 conformant, handles all JSON edge cases (Unicode NFC, number precision, key sorting on non-ASCII).
  - **Golden-file test.** `tests/fixtures/jcs-golden.json` → `tests/fixtures/jcs-golden.canonical` → known ed25519 signature. Test runs on every CI and fails any subtle drift.
  - **Usage.** Every signed artifact (fleet.resolved, probe output) is canonicalized via this single library before signing and before verification. No ad-hoc serializers.
  ```

  Replace with:
  ```markdown
  - **Library choice.** Pinned to [`serde_jcs`](https://crates.io/crates/serde_jcs) `0.2`, hosted by `crates/nixfleet-canonicalize`. Rationale: direct RFC 8785 implementation over `serde_json::Value`; handles UTF-16 key sorting and ECMAScript number formatting per spec. Any change to this pin is a contract change (§VII) requiring signoff from every stream that signs or verifies artifacts (A, B, C).
  - **Golden-file test.** `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.{json,canonical}` with byte-exact equality asserted in `tests/jcs_golden.rs`. Runs on every push via pre-push `cargo nextest run --workspace`; fails loudly on any drift. The ed25519-signed-bytes extension of this fixture lands alongside the CI release key.
  - **Usage.** Every signed artifact (fleet.resolved, probe output) is canonicalized via this single library before signing and before verification. No ad-hoc serializers in Nix, shell, or other crates.
  ```

- [ ] **Step 2.** Verify the TBD in §III is gone.

  Run: `awk '/^## III\./,/^## IV\./' docs/CONTRACTS.md | grep -c TBD`
  Expected: `0`

  Run: `grep -n "serde_jcs" docs/CONTRACTS.md`
  Expected: one match inside §III citing version `0.2`.

- [ ] **Step 3.** Commit.

  ```bash
  git add docs/CONTRACTS.md
  git commit -m "docs(contracts): pin JCS library to serde_jcs 0.2 (§III)"
  ```

---

## Task 9 — Deferred workspace-deps hygiene → PR description

`TODO.md` was removed from tracking in an earlier OSS-polish commit and is listed in `.gitignore`. Reintroducing a tracked `TODO.md` reverses that prior decision and is out of scope here. Instead, the deferred work is captured in the PR description at ship time.

- [ ] **Step 1.** No repository edit. Carry the deferred note into the PR body at ship time:

  > **Deferred.** Migrate shared crate dependencies (serde, serde_json, chrono, anyhow, tracing) to `[workspace.dependencies]` so versions are pinned in one place. Not in scope for this kickoff PR; natural home is the Phase 4 trim pass.

No commit for this task.

---

## Task 10 — Pre-commit conformance

Ensure treefmt normalization is clean before the pre-push gauntlet.

- [ ] **Step 1.** Run project formatter.

  **[USER RUNS]**: `nix fmt -- --no-cache`
  Expected: no files changed (exits cleanly).

- [ ] **Step 2.** Run the `--fail-on-change` variant the pre-commit hook uses.

  **[USER RUNS]**: `nix fmt -- --no-cache --fail-on-change`
  Expected: exit 0.

- [ ] **Step 3.** If Step 1 reformatted any files, commit the normalization; otherwise skip.

  ```bash
  git status
  # If dirty:
  git add -A
  git commit -m "chore(fmt): treefmt normalization"
  ```

---

## Task 11 — Pre-push gauntlet simulation (workspace)

Proves the new crate does not break any existing crate, and the eval side still evaluates.

- [ ] **Step 1.** Workspace test gauntlet.

  **[USER RUNS]**: `nix develop --command cargo nextest run --workspace 2>&1 | tail -40`
  Expected: last line resembles `Summary [N.Ns] X tests run: X passed (N skipped), 0 failed`. Zero failures required.

  If a pre-existing crate fails for unrelated reasons, flag it to the user and stop — do NOT suppress.

- [ ] **Step 2.** Nix eval checks (other half of pre-push hook).

  **[USER RUNS]**:
  ```bash
  for check in eval-hostspec-defaults eval-ssh-hardening eval-username-override eval-locale-timezone eval-ssh-authorized eval-password-files; do
    nix build ".#checks.x86_64-linux.$check" --no-link || { echo "FAILED: $check"; break; }
  done
  ```
  Expected: no `FAILED:` output. These live in Stream B territory — failures block push but are unrelated to Stream C code.

No commit — verification only.

---

## Ship checkpoint

After Tasks 0–11 all green on the user's machine:

- [ ] **Step 1.** Show what will be pushed.

  ```bash
  git -c core.pager=cat log --oneline main..HEAD
  git -c core.pager=cat diff --stat main..HEAD
  ```

- [ ] **Step 2.** Present to user. Exact wording:

  > Branch `feat/12-canonicalize-jcs-pin` ready. N commits ahead of main; diff-stat above. User gauntlet green. Push to `origin` and open PR on `abstracts33d/nixfleet`?

  **Do NOT push, do NOT open PR, do NOT merge without explicit user confirmation.**

- [ ] **Step 3.** When — and only when — the user says ship, push and open PR:

  ```bash
  git push -u origin feat/12-canonicalize-jcs-pin
  gh pr create --repo abstracts33d/nixfleet --base main \
    --title "feat(canonicalize): pin JCS library and ship nixfleet-canonicalize (#12)" \
    --body "$(cat <<'EOF'
  ## Summary
  - New crate `crates/nixfleet-canonicalize`: lib + thin binary, JCS (RFC 8785) canonicalizer pinned to `serde_jcs = "0.2"`.
  - `docs/CONTRACTS.md §III` amended to record the pin — load-bearing contract change per §VII.
  - Golden fixture + four tests (byte-exact match, idempotence, reorder invariance, invalid-JSON rejection).
  - `TODO.md` flags the deferred `[workspace.dependencies]` hygiene pass.

  Partial close of #12 (Rust portion: canonicalize tool). Signature-verification code lands alongside the CI release key handoff from Stream A.

  ## Test plan
  - [x] `cargo test -p nixfleet-canonicalize` green (4 tests).
  - [x] `cargo nextest run --workspace` green (pre-push gate simulated locally).
  - [x] `nix fmt -- --no-cache --fail-on-change` clean.
  - [x] Shell round-trip: `echo '{"b":1,"a":2}' | nixfleet-canonicalize` prints `{"a":2,"b":1}` with no trailing newline.
  - [ ] Reviewer: re-run the workspace test gauntlet locally.

  ## Contract change
  `docs/CONTRACTS.md §III` is amended to pin the JCS library. Per §VII this requires signoff from every stream that signs or verifies. Streams A (CI signer) and B (Nix eval → pre-sign canonicalize) consume the pin; both streams are tagged for review.
  EOF
  )"
  ```

---

## Self-review

- [x] Spec Goal #1 (pin JCS) → Task 1 + Task 8.
- [x] Spec Goal #2 (ship crate with lib + bin) → Tasks 1, 3, 7.
- [x] Spec Goal #3 (lock pin in §III) → Task 8.
- [x] Spec Goal #4 (byte-exact golden test) → Tasks 2–6.
- [x] Every Non-Goal in spec is NOT a task here (no proto, no workspace-deps, no RFC 8785 vectors, no `lib/`/`modules/`/`spike/` touches, no signature verification).
- [x] Test Strategy ordering (RED golden → GREEN lib → idempotence → reorder → invalid → REFACTOR bin → LAST contract edit) is the literal order of Tasks 2–8.
- [x] Both Open Questions are preserved as deferrals, not silently resolved.
- [x] No `TBD`, `later`, or "similar to task N" placeholders.
- [x] Every `Run:` step has expected output.
- [x] Heavy commands (`cargo nextest run --workspace`, `nix fmt`, `nix build`, `nix develop`) are tagged **[USER RUNS]**; cheap per-crate commands are not.
- [x] Branch rename is a pre-flight task before any file changes.
- [x] Ship checkpoint is explicit: no push, no PR without user approval.

# feat/12-canonicalize-jcs-pin — Design Spec

**Date:** 2026-04-24
**Status:** Draft
**Issue:** abstracts33d/nixfleet#12 (Rust portion — canonicalize tool only)
**Branch:** `feat/12-canonicalize-jcs-pin` (rename from `feat/12-canonicalize-proto`)
**Worktree:** `.worktrees/stream-c`
**Stream:** C (Rust). Parallel: Stream B lives in `.worktrees/mkfleet-promotion`.

## Goals

- Pin the JCS (RFC 8785) library for the whole v0.2 fleet to `serde_jcs = "0.2"`.
- Ship a new `crates/nixfleet-canonicalize/` crate exposing `pub fn canonicalize(input: &str) -> Result<String, anyhow::Error>` and a thin `stdin → canonicalize → stdout` binary, shell-invocable by Stream A's CI.
- Lock the pin in `docs/CONTRACTS.md §III` so every future signer (Stream A CI) and verifier (Stream C reconciler + agent fallback) produces byte-identical canonical bytes.
- Prove byte-exactness with a golden-file test committed alongside the lib.

## Non-Goals

- **No `nixfleet-proto` crate.** Types without their first consumer cannot be validated. Proto lands with the reconciler in `feat/3-reconciler-proto`. The canonicalize tool operates on untyped `serde_json::Value` and does not need proto types.
- **No `[workspace.dependencies]` refactor.** Existing v0.1 crates declare per-crate deps; the new crate follows that convention. A workspace-hygiene pass is deferred to Phase 4 trim; a one-line entry will be added to `TODO.md`.
- **No RFC 8785 Appendix E vectors.** Unicode/number/escape edge-case corpus is a follow-up test-only PR, not blocking this one.
- **No touching `lib/`, `modules/`, `spike/`, `crates/agent`, `crates/cli`, `crates/control-plane`, `crates/shared`.** Stream B is live in some of those directories.
- **No signature verification code.** That lands with the reconciler.

## Approach

1. **Single crate, lib + thin bin.** `crates/nixfleet-canonicalize/` with `src/lib.rs` (the function) and `src/main.rs` (the wrapper). Follows the agent/cli layout.
2. **JCS library pinned to `serde_jcs = "0.2"`.** Verified on crates.io: latest stable 0.2.0, exposes `to_string` / `to_vec` / `to_writer` over any `Serialize`, including `serde_json::Value`.
3. **API.** `pub fn canonicalize(input: &str) -> anyhow::Result<String>` — parses the input as `serde_json::Value`, re-emits via `serde_jcs::to_string`. Input-level JSON errors surface as `anyhow::Error` with context.
4. **Binary.** `src/main.rs` reads all of stdin, calls `canonicalize`, writes result to stdout with no trailing newline. Exit code 1 on any error, with the message on stderr.
5. **Conventions.** `edition = "2021"`, `license = "MIT"`, `authors = ["nixfleet contributors"]`, matching existing crates. Deps: `serde_jcs = "0.2"`, `serde_json = "1"`, `anyhow = "1"`. Dev-deps: none required for milestone 1.
6. **Contract amendment.** `docs/CONTRACTS.md §III` TBD block is replaced with: library pin (`serde_jcs 0.2`), rationale, golden-fixture path, and an explicit note that changing the pin is a `contract-change` PR per §VII.
7. **Strict TDD.** Tests land before code per `superpowers:test-driven-development`. See Test Strategy.

## API / Interface

```rust
// crates/nixfleet-canonicalize/src/lib.rs
pub fn canonicalize(input: &str) -> anyhow::Result<String>;
```

Binary (`nixfleet-canonicalize`): reads stdin to EOF, writes canonical JSON to stdout without trailing newline, non-zero exit on error.

Golden fixture (the one hand-verifiable case this PR commits):

- Input (`tests/fixtures/jcs-golden.json`):
  `{"b":2,"a":{"z":null,"y":true,"x":[3,1,2]},"schemaVersion":1}`
- Expected canonical (`tests/fixtures/jcs-golden.canonical`, 61 bytes, no trailing newline):
  `{"a":{"x":[3,1,2],"y":true,"z":null},"b":2,"schemaVersion":1}`

Properties covered by the fixture: top-level key sort, nested key sort, array order preservation (NOT sorted), `null`/`true`/integer literals.

## Edge Cases

- **Invalid JSON input.** `canonicalize("not json")` returns `Err`. Binary prints the error to stderr and exits non-zero.
- **Empty string.** Treated as invalid JSON. Same as above.
- **Trailing newline on input.** `serde_json::from_str` accepts trailing whitespace; output still has no trailing newline.
- **Nested null / boolean / integer literals.** Covered by the golden fixture.
- **Deeply nested / large / Unicode-heavy payloads.** Not exercised in this PR; deferred to the follow-up RFC 8785 Appendix E corpus (see Open Questions).

## Test Strategy

All tests live in `tests/jcs_golden.rs`; fixtures under `tests/fixtures/`. Red-green-refactor, in this order:

1. **RED — golden.** Write `tests/jcs_golden.rs` with `fn golden()` asserting `canonicalize(include_str!("fixtures/jcs-golden.json"))? == include_str!("fixtures/jcs-golden.canonical")`. `canonicalize` does not yet exist → compile error.
2. **GREEN — lib.** Write `src/lib.rs` delegating to `serde_jcs::to_string(&serde_json::from_str::<serde_json::Value>(input)?)`. Golden test passes.
3. **RED — idempotence.** Add `fn idempotent()`: `canonicalize(&canonicalize(x)?)? == canonicalize(x)?`. Should already pass; the test locks in the property.
4. **RED — reorder invariance.** Add `fn reorder_invariance()`: two inputs whose keys are permuted produce identical canonical output.
5. **RED — invalid JSON rejection.** Add `fn rejects_invalid()`: `canonicalize("{").is_err()`.
6. **REFACTOR — binary.** Write `src/main.rs` reading stdin and emitting to stdout. No dedicated binary test in this PR (shell-level smoke tests are Stream A's concern on integration).
7. **LAST — contract pin.** Only after all four tests are green, update `docs/CONTRACTS.md §III` to replace TBD with the working pin.

Acceptance criteria for the PR:
- `cargo test -p nixfleet-canonicalize` green (user-run).
- `cargo nextest run --workspace` (pre-push gate) green — the new crate compiles standalone without touching existing crates.
- `docs/CONTRACTS.md §III` no longer contains "TBD".

**Build economy.** The implementing agent MUST NOT run `cargo test --workspace`, `cargo nextest run --workspace`, `nix build`, or `nix flake check`. Per-crate commands (`cargo fmt -p nixfleet-canonicalize`, `cargo test -p nixfleet-canonicalize`) are handed to the user as copy-paste blocks.

## Files

| File | Action | Purpose |
|------|--------|---------|
| `crates/nixfleet-canonicalize/Cargo.toml` | create | Crate manifest, pinned deps |
| `crates/nixfleet-canonicalize/src/lib.rs` | create | `pub fn canonicalize` |
| `crates/nixfleet-canonicalize/src/main.rs` | create | stdin→stdout wrapper |
| `crates/nixfleet-canonicalize/tests/jcs_golden.rs` | create | Golden + idempotence + reorder + invalid tests |
| `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.json` | create | Input fixture |
| `crates/nixfleet-canonicalize/tests/fixtures/jcs-golden.canonical` | create | Expected output (61 bytes, no trailing newline) |
| `docs/CONTRACTS.md` | edit | §III: replace TBD with pinned `serde_jcs 0.2` + rationale + §VII note |
| `Cargo.lock` | auto | Regenerated by cargo on first build |
| `TODO.md` | edit (append one line) | Flag deferred workspace-deps hygiene pass |

## Open Questions

1. **Option-field null-vs-skip posture for `fleet.resolved`.** Whether `Option<T>::None` fields serialize as `null` or are skipped affects canonical bytes. This is a reconciler-crate decision (types and their serde attributes live with `nixfleet-proto` in `feat/3-reconciler-proto`); the canonicalize tool is untyped and unaffected. Deferred to that PR.
2. **RFC 8785 Appendix E conformance corpus.** A dedicated test-only follow-up PR will add Unicode normalization, float-edge, and escape vectors from the RFC. Not blocking milestone 1.

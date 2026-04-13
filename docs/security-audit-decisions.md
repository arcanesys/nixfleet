# Security Audit Decisions -- Phase 3A

**Date:** 2026-04-14
**Scope:** nixfleet repository (f0c6132)
**Full findings report:** docs/security-audit-results.md

---

## MEDIUM Findings (fixed)

### M1. rustls-pemfile crate unmaintained

- **Location:** control-plane/Cargo.toml:26, control-plane/src/tls.rs
- **Advisory:** RUSTSEC-2025-0134
- **Issue:** `rustls-pemfile` is unmaintained. PEM parsing functionality has been merged into `rustls-pki-types` which was already a direct dependency.
- **Fix:** Migrated `tls.rs` to use `rustls_pki_types::pem::PemObject` trait (`CertificateDer::pem_file_iter`, `PrivateKeyDer::from_pem_file`). Removed `rustls-pemfile` from direct dependencies. The crate remains as a transitive dependency of `axum-server` until that crate updates.
- **Commit:** fix(control-plane): migrate rustls-pemfile to rustls-pki-types PemObject

---

## LOW Findings (deferred to post-launch)

### L1. Agent metrics listener binds 0.0.0.0

- **Location:** agent/src/metrics.rs:18
- **Issue:** Metrics HTTP listener binds all interfaces without a configurable bind address.
- **Rationale for deferral:** Agent runs on fleet nodes where binding to all interfaces is acceptable for Prometheus scraping. The port is already configurable. Adding `--metrics-bind` is a feature addition.

### L2. Agent SQLite database default permissions

- **Location:** agent/src/store.rs:24-27
- **Issue:** Database created via `Connection::open` without explicit permissions. Relies on systemd `StateDirectory=`.
- **Rationale for deferral:** NixOS module sets `StateDirectory = "nixfleet"` which handles directory ownership. Database contains deployment events, not secrets.

### L3. Control-plane SQLite database default permissions

- **Location:** control-plane/src/db.rs:22-28
- **Issue:** Same as L2. Database contains API key hashes and audit events.
- **Rationale for deferral:** Same as L2.

### L4. number_prefix crate unmaintained

- **Location:** Cargo.lock (transitive via indicatif)
- **Advisory:** RUSTSEC-2025-0119
- **Rationale for deferral:** Transitive dependency of `indicatif` (progress bars). Feature-complete, no security implications. Resolves when indicatif updates.

### L5. rand unsound with custom logger

- **Location:** Cargo.lock
- **Advisory:** RUSTSEC-2026-0097
- **Rationale for deferral:** Requires a custom logger that panics inside `rand::rng()`. nixfleet uses `tracing-subscriber` which does not trigger this. Theoretical only.

### L6. CLI push hook shell string replacement

- **Location:** cli/src/release.rs:296
- **Issue:** Hook template uses `.replace("{}", store_path)` then `sh -c`. Store path comes from `nix build` output.
- **Rationale for deferral:** CLI is an operator tool. Hook template is operator-authored. Store paths are trusted Nix output containing only `[a-z0-9._+-]`.

### L7. SSH StrictHostKeyChecking=accept-new

- **Location:** cli/src/host.rs:41-42
- **Issue:** Uses TOFU for initial host provisioning.
- **Rationale for deferral:** Correct behavior for `nixfleet host add`. Accepts unknown keys on first connect, rejects changes on subsequent connects.

### L11. Flake inputs from non-trusted-list orgs

- **Location:** flake.nix
- **Issue:** `vic/import-tree`, `astro/microvm.nix`, and `LnL7/nix-darwin` are not on the predefined trusted-org list for this audit.
- **Rationale for deferral:** All three are canonical community projects with wide adoption. `import-tree` is a minimal pure-Nix utility. `microvm.nix` is the standard NixOS microVM framework. `nix-darwin` is the only macOS Nix management tool. The trusted-org list should be expanded to include them.

### L12. Mutable branch refs in flake inputs

- **Location:** flake.nix (darwin/master, nixos-hardware/master)
- **Issue:** Two inputs use mutable `master` branch refs instead of tagged releases.
- **Rationale for deferral:** Standard practice for nix-darwin and nixos-hardware, which do not publish release tags. The lock file pins each to a specific revision, so builds remain reproducible.

### L13. nixfleet-demo stale nixfleet pin (cross-repo)

- **Location:** nixfleet-demo/flake.lock
- **Issue:** Demo pins nixfleet rev d8502d72 (2026-04-05) which still includes the now-removed attic input.
- **Rationale for deferral:** The lock still pins attic to a specific rev -- reproducibility is maintained. Resolves with `nix flake update nixfleet` in the demo repo. No changes were needed in nixfleet-demo, so no branch was created there.

---

## ACCEPTED (false positive / intentional / out of scope)

### SQL injection -- CLEAN

All SQL in `control-plane/src/db.rs` and `agent/src/store.rs` uses `rusqlite` parameterized statements. `format!()` calls build only `?N` placeholder tokens.

### Unsafe blocks -- CLEAN

Zero occurrences across all four crates.

### TLS verification -- CLEAN

No instances of disabled TLS verification. Proper `WebPkiClientVerifier` configuration.

### Command injection -- CLEAN

Store paths validated with strict allowlist. SSH arguments passed via `Command::new().args()`. Health check commands from Nix-deployed root-owned config.

### Cargo audit -- CLEAN

Zero known CVEs.

### Sensitive data -- CLEAN

All keyword matches are variable names, documentation examples, or test fixtures.

### Flake inputs -- CLEAN

All direct inputs from trusted or well-known community sources. Lock files within 3-week freshness window.

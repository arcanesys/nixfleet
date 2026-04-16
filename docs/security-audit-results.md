# Phase 3A: Security Audit Results

**Date:** 2026-04-14
**Auditor:** Claude (automated + manual review)
**Repos:** nixfleet (f0c6132), nixfleet-compliance (f385ad7), nixfleet-demo (3454e6b)

---

## Summary

| Severity | Count | Action |
|----------|:-----:|--------|
| BLOCKER  | 0     | --     |
| HIGH     | 0     | --     |
| MEDIUM   | 4     | Fix on branch |
| LOW      | 13    | Deferred to post-launch |
| ACCEPTED | 20+   | Documented below |

**No blockers or high-severity findings.** Four medium findings require fixes before launch.

---

## MEDIUM Findings (fixed on branch)

### M1. rustls-pemfile crate unmaintained (nixfleet)

- **Location:** control-plane/Cargo.toml:26, control-plane/src/tls.rs
- **Type:** dependency-risk
- **Advisory:** RUSTSEC-2025-0134
- **Issue:** `rustls-pemfile` is unmaintained. PEM parsing is now in `rustls-pki-types` (already a dependency).
- **Fix:** Migrate to `rustls_pki_types::pem::PemObject` API, remove `rustls-pemfile` dependency.

### M2. mkProbe missing `set -eu` shell safety (nixfleet-compliance)

- **Location:** lib/mkProbe.nix:28
- **Type:** shell-safety
- **Issue:** `mkProbe` wraps all 16 probe scripts but only sets `set -o pipefail`, omitting `-e` (exit on error) and `-u` (unset variable is error). An unset variable silently expands to empty string.
- **Fix:** Change to `set -euo pipefail`.

### M3. Hand-built JSON via string concatenation (nixfleet-compliance)

- **Location:** controls/_encryption-in-transit.nix:71-76
- **Type:** json-injection
- **Issue:** Expiring certificate list is built via `"$expiring_list{\"domain\":\"$domain\",\"days_left\":$days_left},"` -- shell string concatenation. A domain directory containing a double-quote character would break the JSON and could inject arbitrary content into evidence output.
- **Fix:** Use `jq --arg` to build each entry safely.

### M4. Incomplete systemd hardening on evidence collector (nixfleet-compliance)

- **Location:** evidence/collector.nix:47-49
- **Type:** systemd-hardening
- **Issue:** The collector service has `NoNewPrivileges`, `ProtectHome`, and `PrivateTmp` but lacks further hardening. Service runs as root by default.
- **Fix:** Add `ProtectSystem`, `ProtectKernelTunables`, `ProtectControlGroups`, `ProtectKernelModules`, `RestrictNamespaces`, `MemoryDenyWriteExecute`, `LockPersonality`, `RestrictRealtime` with appropriate `ReadWritePaths`.

---

## LOW Findings (deferred to post-launch)

### L1. Agent metrics listener binds 0.0.0.0 (nixfleet)

- **Location:** agent/src/metrics.rs:18
- **Issue:** Metrics HTTP listener binds all interfaces. Port is configurable but bind address is not.
- **Rationale for deferral:** Agent runs on fleet nodes where binding to all interfaces is generally acceptable for Prometheus scraping. Adding a `--metrics-bind` flag is a feature addition, not a security fix.

### L2. Agent SQLite database default permissions (nixfleet)

- **Location:** agent/src/store.rs:24-27
- **Issue:** Database created with default umask. Relies on systemd `StateDirectory=` for correct ownership.
- **Rationale for deferral:** The NixOS module sets `StateDirectory = "nixfleet"`, which creates the directory with correct ownership. The database contains deployment events, not secrets. Explicit `0600` in Rust code would be defense-in-depth but not urgent.

### L3. Control-plane SQLite database default permissions (nixfleet)

- **Location:** control-plane/src/db.rs:22-28
- **Issue:** Same pattern as L2. Database contains API key hashes and audit events.
- **Rationale for deferral:** Same as L2 -- systemd `StateDirectory=` handles directory permissions.

### L4. number_prefix crate unmaintained (nixfleet)

- **Location:** Cargo.lock (transitive via indicatif)
- **Advisory:** RUSTSEC-2025-0119
- **Rationale for deferral:** Transitive dependency. Feature-complete crate with no security implications. Resolves when `indicatif` updates.

### L5. rand unsound with custom logger (nixfleet)

- **Location:** Cargo.lock (direct + transitive)
- **Advisory:** RUSTSEC-2026-0097
- **Rationale for deferral:** Unsoundness requires a custom logger that panics inside `rand::rng()`. nixfleet uses `tracing-subscriber` which does not panic in this path. Will be fixed in a future `rand` patch.

### L6. CLI push hook shell string replacement (nixfleet)

- **Location:** cli/src/release.rs:296
- **Issue:** Push hook template uses `.replace("{}", store_path)` then passes to `sh -c`. Store path comes from `nix build --print-out-paths` (trusted).
- **Rationale for deferral:** CLI is an operator tool. Hook template is operator-authored. Store paths are trusted Nix output. Risk is inherent to the feature design.

### L7. SSH StrictHostKeyChecking=accept-new (nixfleet)

- **Location:** cli/src/host.rs:41-42
- **Issue:** Uses `accept-new` (TOFU) for initial host provisioning.
- **Rationale for deferral:** This is correct TOFU behavior for `nixfleet host add`. It accepts unknown keys on first connect but rejects changed keys subsequently. Different from the dangerous `StrictHostKeyChecking=no`.

### L8. probe-runner.sh hand-built JSON in meta fallback (nixfleet-compliance)

- **Location:** evidence/probe-runner.sh:30
- **Issue:** `meta="{\"control\": \"${control_name}\", ...}"` -- hand-built JSON. `control_name` comes from Nix-built filenames.
- **Rationale for deferral:** Attack surface is extremely narrow (requires malicious Nix option value). Low impact since the meta fallback path is rarely hit (`.meta` files are always generated by the linkFarm).

### L9. probe-runner.sh hand-built JSON in error fallback (nixfleet-compliance)

- **Location:** evidence/probe-runner.sh:60
- **Issue:** `checks="{\"error\": \"probe exited with code $?\"}"` -- hand-built JSON. `$?` is always numeric.
- **Rationale for deferral:** No injection possible since `$?` is an integer. Style inconsistency only.

### L10. baseline-hardening double-quoted awk program (nixfleet-compliance)

- **Location:** controls/_baseline-hardening.nix:113
- **Issue:** `awk "BEGIN {printf \"%.2f\", $passed / $total}"` -- variables expanded inside awk string.
- **Rationale for deferral:** Both `$passed` and `$total` are shell integers computed locally. No external input path. Anti-pattern but no practical risk.

### L11. Flake inputs from non-trusted-list orgs (nixfleet)

- **Location:** flake.nix
- **Issue:** `vic/import-tree`, `astro/microvm.nix`, `LnL7/nix-darwin` are not on the predefined trusted-org list.
- **Rationale for deferral:** All three are canonical community projects with wide adoption. The trusted-org list should be expanded to include them. No actual security risk.

### L12. Mutable branch refs in flake inputs (nixfleet)

- **Location:** flake.nix (darwin/master, nixos-hardware/master)
- **Issue:** Use mutable `master` branch refs.
- **Rationale for deferral:** Standard practice for projects that do not publish release tags. Lock file pins to specific revs.

### L13. nixfleet-demo stale nixfleet pin with orphaned attic input

- **Location:** nixfleet-demo/flake.lock
- **Issue:** Demo pins nixfleet rev d8502d72 (2026-04-05) which still includes the now-removed attic input.
- **Rationale for deferral:** Lock still pins attic to a specific rev. Reproducibility is maintained. Resolves with `nix flake update nixfleet` in the demo repo.

---

## ACCEPTED Findings (false positive / intentional / out of scope)

### Sensitive Data Scan -- All Repos

All keyword matches (password, secret, token, api_key, credential) across all 3 repos are:
- **Variable/option names** in Nix `mkOption` declarations and Rust struct fields
- **Documentation examples** in mdbook guides, ADRs, CLI help text
- **Test fixtures** in VM tests (throwaway passwords, SSH keys, API keys clearly scoped to `#[test]` or NixOS VM tests with comments explaining their disposable nature)
- **Intentional demo data** in nixfleet-demo (age-identity.txt, *.age files, documented with production migration instructions)

No hardcoded production credentials found in any repo.

### Private IPs and Hostnames

- **192.168.1.x** in documentation/CLI help: Generic RFC 1918 placeholder IPs, standard documentation practice.
- **10.42.0.x** in microVM bridge defaults: Configurable defaults for isolated virtual bridges, not infrastructure IPs.
- **10.0.100.x** in nixfleet-demo: Intentional QEMU VLAN addresses for the 6-host demo fleet.
- **`.fleet.internal`** in examples/: Fictional placeholder hostname in example modules.

### Credential Files

- **fleet-ca.pem** (demo): Public CA certificate, safe to commit.
- **attic-signing-key.pub** (demo): Public key, safe to commit.
- **age-identity.txt** (demo): Intentional demo private key with explicit README warnings and production migration instructions.
- **\*.age files** (demo): Age-encrypted secrets, standard agenix workflow.

### SQL Injection -- CLEAN

All SQL in `control-plane/src/db.rs` and `agent/src/store.rs` uses `rusqlite` parameterized statements (`?N` placeholders). The `format!()` calls build only placeholder tokens, never user data.

### Unsafe Blocks -- CLEAN

Zero occurrences across all four Rust crates.

### TLS Verification -- CLEAN

Zero instances of disabled TLS verification. Proper `WebPkiClientVerifier` configuration. Agent rejects `http://` URLs unless `--allow-insecure` is explicitly set.

### Command Injection -- CLEAN

- Store paths validated with strict allowlist (`validate_store_path`)
- SSH arguments passed via `Command::new().args()` (not shell interpolation)
- Health check commands come from Nix-deployed root-owned config files (equivalent to any NixOS system config)

### Cargo Audit -- CLEAN

Zero known CVEs. 366 dependencies scanned against 1043 advisories.

### Flake Inputs -- CLEAN

All direct inputs from trusted or well-known community sources. All lock files within 3-week freshness window.

### Compliance Probes -- CLEAN

- No secrets or sensitive data collected by any of the 16 probes
- All probes have graceful failure handling (`|| true`, default values)
- All final jq output uses `--arg`/`--argjson` (not string interpolation)
- probe-runner.sh has correct `set -euo pipefail` and proper variable quoting

### Demo Configuration -- CLEAN

- recipients.nix contains only the demo key
- No real hostnames or IPs in any host/module config
- SSH key is a clear placeholder ("NixfleetDemoKeyReplaceWithYourOwn")
- All 15 .age files decrypt with the demo key (verified)
- README has appropriate security warnings and production migration guide

### Internal References -- CLEAN

Zero references to Slack, Linear, Jira, Notion, Confluence, or digitpro across all 3 repos. One match for "linear" is the English adjective in an ADR.

### Transitive Flake Dependencies

Transitive inputs from `ipetkov/crane`, `cachix/pre-commit-hooks.nix`, `edolstra/flake-compat`, `oxalica/rust-overlay`, `Enzime/nix-vm-test`, `spectrum-os.org` are all well-known community projects pulled in by trusted direct inputs. Cannot be controlled without forking upstream.

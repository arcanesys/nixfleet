# Operator Cert Mint + Trust-Bootstrap De-Migration — Design Spec

**Date:** 2026-05-08
**Repos affected:** `nixfleet` (most), `fleet` (one wiring change)
**Branch:** `feat/operator-cert-mint`

## Goal

Give every nixfleet operator workstation a clean way to mint its own
mTLS client cert from the offline fleet root CA, so that the operator
CLI (`nixfleet status`, `nixfleet rollout trace`) authenticates with a
distinct identity rather than reusing the agent's SSH-derived cert.
Bundle the work with cleanup of dev-transitional language ("Bundle C",
`#41`, MIGRATION.md) that's no longer needed now that the v0.1→v0.2
trust hierarchy migration has shipped.

## Non-goals

- Network enrollment flow (operator hits a `/v1/operators/enroll`
  endpoint). The offline-root model is what the trust hierarchy was
  built for; a network flow would be a separate RFC.
- Multi-operator key sharing / agent-forward-style workflows. Each
  operator workstation mints its own cert from its local copy of the
  offline root key.
- Operator-cert revocation tooling. The CP's existing CN-based
  revocation flow already accepts `operator-*@*` CNs unchanged; surfacing
  a `nixfleet operator revoke` UX is a separate piece.
- Bash port to Rust for `tools/trust-bootstrap/`. The shell tool stays
  shell; only its name and labelling change in this PR.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Operator workstation (krach)                                   │
│                                                                 │
│  ~/.config/nixfleet/                                            │
│    fleet-root.cert.pem  ← already there                         │
│    fleet-root.key.pem   ← already there (OFFLINE)               │
│                                                                 │
│  Run once per workstation, then yearly:                         │
│  ┌────────────────────────────────────────────────┐             │
│  │  nixfleet-mint-operator-cert  (NEW BIN)        │             │
│  │   reads:  fleet-root.cert.pem                  │             │
│  │           fleet-root.key.pem                   │             │
│  │   mints:  ECDSA-P-256 keypair                  │             │
│  │           X.509 cert, CN=operator-s33d@krach,  │             │
│  │           clientAuth EKU, 365d validity        │             │
│  │           signed by fleet-root.key.pem         │             │
│  │   writes: operator.pem (0644)                  │             │
│  │           operator.key (0600)                  │             │
│  └────────────────────────────────────────────────┘             │
│                                                                 │
│  Then:                                                          │
│  ┌────────────────────────────────────────────────┐             │
│  │  nixfleet config init  (existing, unchanged)   │             │
│  │   --client-cert ~/.config/nixfleet/operator.pem│             │
│  │   --client-key  ~/.config/nixfleet/operator.key│             │
│  │   writes config.toml                           │             │
│  └────────────────────────────────────────────────┘             │
└─────────────────────────────────────────────────────────────────┘
            │
            │   nixfleet status, etc.
            ▼
┌─────────────────────────────────────────────────────────────────┐
│  Lab CP (no changes required)                                   │
│   trust roots already include fleet-root + issuance-ca          │
│   require_cn middleware accepts any non-revoked CN              │
│   operator-s33d@krach passes through (not the agent CN guard)   │
└─────────────────────────────────────────────────────────────────┘
```

The bin is pure offline crypto — it never opens a socket. The CP needs
zero changes: any cert chained to the existing root passes mTLS, and
the existing CN-revocation flow accepts operator CNs unchanged.

## Components

### 1. `crates/nixfleet-cli/src/operator_cert.rs` (new module)

Pure lib code, ~120 LOC. No `std::io` at the public boundary except
for atomic-write of the outputs.

```rust
pub struct MintOperatorCertArgs {
    pub root_cert_path: PathBuf,
    pub root_key_path: PathBuf,
    pub cn: String,
    pub output_cert_path: PathBuf,
    pub output_key_path: PathBuf,
    pub validity_days: u32,
    pub overwrite: bool,
}

pub struct MintOutcome {
    pub cn: String,
    pub not_after: chrono::DateTime<chrono::Utc>,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub fn mint_operator_cert(args: MintOperatorCertArgs) -> Result<MintOutcome>;
```

**Algorithm:** ECDSA-P-256 (matches the offline root, native to
`aws-lc-rs`, supported by reqwest+rustls without extra features).

**Cert shape:**
- `Subject` = `CN=<args.cn>, O=arcanesys, OU=fleet`
- `BasicConstraints` = `cA:FALSE`
- `KeyUsage` = `DigitalSignature`
- `ExtendedKeyUsage` = `clientAuth`
- `NotBefore` = `now − 5 min` (clock-skew tolerance)
- `NotAfter` = `now + (validity_days × 86400 s)`
- Signed by the provided root key

**Atomic write:** mirrors the `FileConfig::save` pattern from issue #66
work — write to `<path>.tmp.<pid>` in same dir, fsync, rename, chmod.
Cert gets 0644, key gets 0600. On Unix, mode is set via
`OpenOptionsExt`. Non-Unix falls back to `std::fs::write` (NixFleet
targets Linux/Darwin only; both are Unix).

### 2. `crates/nixfleet-cli/src/bin/mint_operator_cert.rs` (new bin)

Thin clap wrapper, ~80 LOC. Default-resolution chain for each path:

| Argument | Resolution chain |
|---|---|
| `--root-cert` | flag → `NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE` → `${XDG_CONFIG_HOME:-${HOME}/.config}/nixfleet/fleet-root.cert.pem` → error if missing |
| `--root-key` | same chain, `KEY_FILE` env, `fleet-root.key.pem` filename |
| `--cn` | flag → `operator-${USER}@${HOSTNAME}`. `${USER}` from `std::env::var`. `${HOSTNAME}` from `whoami::fallible::hostname()`. |
| `--output-cert` | flag → `~/.config/nixfleet/operator.pem` |
| `--output-key` | flag → `~/.config/nixfleet/operator.key` |
| `--days` | flag → `365` |
| `--force` | bool flag, default `false` |

**New runtime dep:** `whoami = "1"` (small, well-maintained, replaces
brittle `gethostname` shell-out).

**Output to stderr** (per workspace convention — stdout reserved for
machine-readable output, stderr for human acknowledgements):

```
minted operator cert
  cn:        operator-s33d@krach
  valid until: 2027-05-08T20:14:00Z (365 days)
  cert:      /home/s33d/.config/nixfleet/operator.pem
  key:       /home/s33d/.config/nixfleet/operator.key

next: nixfleet config init --client-cert <cert> --client-key <key>
```

### 3. `modules/scopes/nixfleet/_operator.nix` (modify existing)

Add two options + env-var exports parallel to the existing
`orgRootKeyFile` pattern:

```nix
fleetRootCertFile = lib.mkOption {
  type = lib.types.nullOr lib.types.str;
  default = null;
  example = "/home/operator/.config/nixfleet/fleet-root.cert.pem";
  description = ''
    Path to the offline fleet root CA cert PEM. Read by
    `nixfleet-mint-operator-cert` to issue per-workstation operator
    certs. Set on operator workstations only — `null` elsewhere.
  '';
};

fleetRootKeyFile = lib.mkOption {
  type = lib.types.nullOr lib.types.str;
  default = null;
  example = "/home/operator/.config/nixfleet/fleet-root.key.pem";
  description = ''
    Path to the offline fleet root CA private key PEM. Read by
    `nixfleet-mint-operator-cert` to issue per-workstation operator
    certs. Set on operator workstations only — `null` elsewhere.
    Never read by any systemd service.
  '';
};
```

In the `config = lib.mkIf cfg.enable { ... }` block:

```nix
environment.variables = lib.filterAttrs (_: v: v != null) {
  NIXFLEET_OPERATOR_ORG_ROOT_KEY = cfg.orgRootKeyFile;
  NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE = cfg.fleetRootCertFile;
  NIXFLEET_OPERATOR_FLEET_ROOT_KEY_FILE = cfg.fleetRootKeyFile;
};
```

(The existing `lib.mkIf (cfg.orgRootKeyFile != null)` shape is replaced
with the `filterAttrs` form so all three vars share one
`environment.variables` declaration.)

### 4. Trust-bootstrap rename + de-migrate

**Renames** (file/dir moves, content updates):

- `tools/cp-bootstrap/` → `tools/trust-bootstrap/`
- `tools/cp-bootstrap/bootstrap.sh` → `tools/trust-bootstrap/bootstrap.sh`
- `tools/cp-bootstrap/default.nix` → `tools/trust-bootstrap/default.nix`
  (package name `nixfleet-cp-bootstrap` → `nixfleet-trust-bootstrap`)
- `tools/cp-bootstrap/MIGRATION.md` → deleted
- New: `tools/trust-bootstrap/README.md` documenting the standing
  function (new-fleet stand-up, annual issuance-CA renewal, TPM
  rotation, disaster recovery). No v0.1→v0.2 narrative.

**`bootstrap.sh` content edits:**

- Strip `Bundle C` / `nixfleet#41` / `Migration` references from
  comments, `usage()`, and the generated `README.txt`
- Default `--output-dir` becomes `${HOME}/.config/nixfleet`
  (flat, no `bundle-c/` subdir). The `--output-dir` flag remains for
  cases where an operator wants the artifacts elsewhere.
- The generated `README.txt` no longer mentions
  "Keep `--fleet-ca-key` for the overlap window" (legacy-overlap
  language).

**`modules/operator-tools.nix`** package rename:

- `packages.nixfleet-cp-bootstrap` → `packages.nixfleet-trust-bootstrap`
- `apps.nixfleet-cp-bootstrap` → `apps.nixfleet-trust-bootstrap`
- Description loses the `Bundle C / nixfleet#41` reference

### 5. Phase-label strip across the codebase

Per `feedback_docs_generic_only` (no Phase/Task/PR/cycle/commit-hash
references in committed text outside `docs/adr/` and
`docs/superpowers/`), the following sites currently violate the rule
and need rewording. Each is replaced with a description of *what*
rather than *when*.

| File | Line(s) | Current | Replace with |
|---|---|---|---|
| `crates/nixfleet-proto/src/trust.rs` | 36, 45 | `Bundle C / #41` | (drop the parenthetical; keep the descriptive sentence) |
| `crates/nixfleet-control-plane/src/server/checkin_pipeline/mod.rs` | 27 | `Bundle C: cert CN may be canonical (...)` | `cert CN may be canonical (...)` |
| `modules/scopes/nixfleet/_trust-json.nix` | 25 | `Bundle C / nixfleet#41:` | (drop label; keep behaviour description) |
| `modules/scopes/nixfleet/_control-plane.nix` | 247, 258, 272, 581 | `Bundle C ...` (4 sites) | (drop labels; keep descriptions) |
| `contracts/trust.nix` | 187, 200 | `Bundle C / nixfleet#41` | (drop labels) |

The `tools/trust-bootstrap/bootstrap.sh` content is handled by the
rename in §4.

### 6. Fleet repo wiring (`fleet`)

**`modules/nixfleet/operator.nix`** (krach-only) gains two literal-path
declarations using the operators primary-name lookup:

```nix
{config, ...}: let
  primaryHome = config.users.users.${config.nixfleet.operators._primaryName}.home;
in {
  nixfleet.operator = {
    enable = true;
    orgRootKeyFile = config.age.secrets.org-root-key.path;
    fleetRootCertFile = "${primaryHome}/.config/nixfleet/fleet-root.cert.pem";
    fleetRootKeyFile  = "${primaryHome}/.config/nixfleet/fleet-root.key.pem";
  };
}
```

This becomes effective after the fleet bumps the nixfleet input to
include the new options, redeploys, and the operator runs the new
mint bin.

## Data flow

```
$ nixfleet-mint-operator-cert
  ├─ resolve root_cert_path:
  │    --root-cert | NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE
  │      | $XDG_CONFIG_HOME/nixfleet/fleet-root.cert.pem
  │      | ~/.config/nixfleet/fleet-root.cert.pem
  ├─ resolve root_key_path:  (same chain, KEY_FILE env)
  ├─ resolve cn:        --cn | "operator-${USER}@${HOSTNAME}"
  ├─ resolve outputs:   --output-cert | ~/.config/nixfleet/operator.pem
  │                     --output-key  | ~/.config/nixfleet/operator.key
  ├─ resolve days:      --days | 365
  └─ call lib::mint_operator_cert(args)

lib::mint_operator_cert flow:
  1. existence + readability check on root files; bail if missing
  2. existence check on outputs:
       output exists ∧ !overwrite → bail with "pass --force"
  3. parse root cert (rcgen::CertificateParams::from_ca_cert_pem)
  4. parse root key  (rcgen::KeyPair::from_pem)
  5. assert root key algorithm = ECDSA-P-256;
     reject otherwise with a clear "expected ECDSA-P-256, got <X>"
  6. generate operator keypair via rcgen (ECDSA-P-256)
  7. build CertificateParams (CN, O, OU, EKU clientAuth, KU
     digitalSignature, BC cA:false, NotBefore now-5min, NotAfter
     now+days)
  8. sign: params.signed_by(&op_key, &root_cert, &root_key)
  9. atomic write outputs (write to tmp, fsync, rename, chmod)
 10. return MintOutcome { cn, not_after, cert_path, key_path }

bin prints MintOutcome formatted to stderr; stdout stays empty.
```

## Error taxonomy

| Condition | Behaviour | Exit |
|---|---|---|
| `--root-cert`/`-key` unresolved (no flag, no env, no file at convention path) | bail: "no fleet root cert at \<resolved path\>; pass --root-cert or set NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE" | 1 |
| Root cert/key file unreadable | bail: "read \<path\>: \<os error\>" | 1 |
| Root key parses but isn't ECDSA-P-256 | bail: "fleet root key must be ECDSA-P-256 (matches issuance CA chain); got \<algorithm\>" | 1 |
| Output exists, no `--force` | bail: "\<path\> already exists; pass --force to overwrite" | 1 |
| Output dir missing | create with mode 0700 | — |
| `--days` ≤ 0 | bail: "validity must be at least 1 day; got \<n\>" | 1 |
| CN empty after defaulting | bail: "operator CN is empty (USER and HOSTNAME both unset?); pass --cn" | 1 |
| Atomic write fails mid-flight | leave temp file in place + bail with the original error | 1 |
| Root cert + root key don't pair | rcgen's `signed_by` fails — surface its error verbatim | 1 |

No retries, no fallbacks. One-shot operator action.

## Concurrency

None. Single-process, no shared state, no daemon.

## Testing

### Lib unit tests in `operator_cert.rs`

| Test | What it verifies |
|---|---|
| `mints_cert_signed_by_provided_root` | Output cert's `issuer` matches root's `subject`; signature verifies against root pubkey. |
| `output_cert_has_correct_cn_and_eku` | CN matches `args.cn`; EKU contains `clientAuth`; KeyUsage contains `DigitalSignature`; `BasicConstraints.cA = false`. |
| `validity_window_matches_days` | `not_before ≈ now-5min`, `not_after ≈ now + days*86400` (±1min). |
| `output_key_pairs_with_output_cert` | Sign a probe with output key, verify with cert pubkey. |
| `refuses_overwrite_without_force` | Pre-existing output → `Err`; bytes unchanged. |
| `overwrites_with_force` | Pre-existing output → succeeds; new outputs replace old. |
| `rejects_non_ecdsa_root_key` | Ed25519 root → `Err` with "expected ECDSA-P-256". |
| `output_key_mode_is_0600_on_unix` | `#[cfg(unix)]` gated; key 0600, cert 0644. |

Each test mints a throwaway root via `rcgen` in a `TempDir`. No
filesystem pollution.

### Bin smoke test (`tests/mint_operator_cert_smoke.rs`)

| Test | What it verifies |
|---|---|
| `bin_resolves_root_paths_via_env` | Sets `NIXFLEET_OPERATOR_FLEET_ROOT_{CERT,KEY}_FILE`, omits flags, asserts success + outputs exist. |

Uses `Command::cargo_bin("nixfleet-mint-operator-cert")`. No CP boot.

### Module tests

The `_operator.nix` option additions are evaluation-checked by
`nix flake check` via the existing eval-test harness. No bespoke
test added.

### Trust-bootstrap rename — verification only

- `nix run .#nixfleet-trust-bootstrap -- --help` exits 0 (manual)
- After the rename, this grep should return zero hits in non-exempt paths:

  ```sh
  grep -RE 'Bundle C|cp-bootstrap|nixfleet#41|MIGRATION\.md' \
    -- modules/ crates/ tools/ contracts/ README.md
  ```

  The phrase "Bundle C" remains permitted in `docs/adr/` and
  `docs/superpowers/` per `feedback_docs_generic_only`.

### Test runtime

Lib tests <1s. Smoke test ~5s after `cargo build`. No CP boot.
Within build-economy budget.

### Out of scope

- Cross-platform key-mode behaviour (Unix only; matches existing CLI)
- rcgen library correctness (assumed valid)
- Live mTLS handshake (manual `nixfleet status` post-mint is the
  integration check; CP-boot for a pure-crypto verification has bad
  cost/value)

## Migration

This change is **additive**: nothing in the existing CLI surface
breaks. Operators who already manually pointed `nixfleet config init`
at an agent cert (or the SSH-derived ad-hoc PKCS#8 from the krach
testing path) can keep doing so. The new flow becomes available when
an operator runs `nixfleet-mint-operator-cert` and re-runs
`nixfleet config init` to point at the operator-cert paths.

For arcanesys's specific case:
1. Land this PR on nixfleet `main`, push to lab.
2. Bump fleet input + add the two `fleetRoot*File` options on krach,
   push to lab.
3. After agents redeploy, on krach:
   - `mv ~/.config/nixfleet/bundle-c/* ~/.config/nixfleet/`
   - `rm -rf ~/.config/nixfleet/bundle-c/`
   - `rm ~/.config/nixfleet/credentials.toml` (V1 test garbage)
   - `nixfleet-mint-operator-cert` (writes `operator.{pem,key}`)
   - `nixfleet config init --force --cp-url https://lab:8080 \
       --ca-cert /etc/nixfleet/fleet-ca.pem \
       --client-cert ~/.config/nixfleet/operator.pem \
       --client-key  ~/.config/nixfleet/operator.key`
   - `nixfleet status`

## Implementation surface

| Area | Files | Approx LOC |
|---|---|---|
| Lib module | `crates/nixfleet-cli/src/operator_cert.rs` | ~120 |
| Bin | `crates/nixfleet-cli/src/bin/mint_operator_cert.rs` | ~80 |
| Cargo deps | `crates/nixfleet-cli/Cargo.toml` (`whoami = "1"`, bin entry) | ~5 |
| Lib re-export | `crates/nixfleet-cli/src/lib.rs` | ~2 |
| Tests | `crates/nixfleet-cli/src/operator_cert.rs#tests`, `crates/nixfleet-cli/tests/mint_operator_cert_smoke.rs` | ~150 |
| Operator scope | `modules/scopes/nixfleet/_operator.nix` | ~30 |
| Trust-bootstrap rename | `tools/trust-bootstrap/{bootstrap.sh,default.nix,README.md}`, `modules/operator-tools.nix`, deletion of `tools/cp-bootstrap/MIGRATION.md` | ~50 net |
| Phase-label strip | `crates/nixfleet-proto/src/trust.rs`, `crates/nixfleet-control-plane/src/server/checkin_pipeline/mod.rs`, `modules/scopes/nixfleet/_trust-json.nix`, `modules/scopes/nixfleet/_control-plane.nix`, `contracts/trust.nix` | ~20 |
| README updates | `crates/nixfleet-cli/Cargo.toml` description, possibly `README.md` if relevant | ~5 |
| Fleet wiring | `fleet/modules/nixfleet/operator.nix` | ~5 |

**Total:** ~470 LOC across two repos. Most of that is the lib + bin
skeleton; the rename + label strip is mechanical.

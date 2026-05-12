# Troubleshooting

Known failure modes from real-hardware testing. Each entry: symptom -> cause -> fix.

## CP service flaps after deploy

**Symptom**: `systemctl status nixfleet-control-plane.service` shows `failed` with `code=killed, signal=TERM`. PID changes every 10s.

**Cause**: agenix entry references a file that doesn't exist in the `secrets` flake input - usually because the agenix recipient list doesn't include `cp` for a secret CP needs.

**Fix**: Run `journalctl -u nixfleet-control-plane.service | grep agenix`. Look for `failed to open input file` or `no identity matched any of the recipients`. Add `cp` to the recipient list in your secrets repo, re-encrypt, push, bump fleet's secrets lock, redeploy.

## SQLite migration error on CP boot

**Symptom**: CP fails to start with `applied migration V1__initial_schema is different than filesystem one V1__initial`.

**Cause**: The DB was initialised by a previous (v0.1) version of the CP that used a different migration filename. Refinery refuses to apply when names diverge.

**Fix**: Wipe the DB; migrations re-apply from scratch. Safe because pending_confirms/token_replay/cert_revocations are not load-bearing across upgrades.
```sh
ssh root@cp "systemctl stop nixfleet-control-plane.service && \
  rm -f /var/lib/nixfleet-cp/state.db /var/lib/nixfleet-cp/state.db-wal \
        /var/lib/nixfleet-cp/state.db-shm && \
  systemctl start nixfleet-control-plane.service"
```

## Forgejo poll fails with TLS handshake error

**Symptom**: `journalctl -u nixfleet-control-plane.service | grep forgejo` shows `forgejo poll failed; retaining previous cache`.

**Cause**: Old behaviour - CP's reqwest client used webpki-roots only, which doesn't include the Caddy local CA. The reqwest build now uses the `rustls-tls-native-roots` feature.

**Fix**: Bump fleet's `nixfleet` input past the fix, redeploy cp.

## Verify fails with `BadSignature` even though the trust public key matches the TPM

**Symptom**: `verify_ok: false reason: BadSignature` on every reconcile tick. Manual signature check confirms the key matches.

**Cause**: Old behaviour - verifier rejected high-s ECDSA signatures (Bitcoin-style strict-low-s). TPM2_Sign emits high-s ~50% of the time. The verifier now normalises high-s before verifying.

**Fix**: Bump fleet's `nixfleet` input past the fix. The verifier now normalises high-s before verifying.

## Agent fails activation with `unrecognized arguments: --system`

**Symptom**: Agent log shows `nixos-rebuild: error: unrecognized arguments: --system /nix/store/...`. Activation halts; no rollback fires.

**Cause**: Old behaviour - agent shelled out to `nixos-rebuild switch --system <path>`. NixOS 26.05's `nixos-rebuild-ng` (Python rewrite) renamed the flag to `--store-path`. The agent now calls `nix-env --profile ... --set` + `<path>/bin/switch-to-configuration switch` directly, bypassing nixos-rebuild's CLI surface entirely.

**Fix**: Bump fleet's `nixfleet` input past the fix, redeploy each host.

## Rollback timer never marks expired rows

**Symptom**: `pending_confirms` rows stay `pending` indefinitely past their `confirm_deadline`.

**Cause**: Old behaviour - query did `WHERE confirm_deadline < datetime('now')`. Stored values are RFC3339 (`T`-separator) but `datetime('now')` returns space-separated. Lex compare put `T` (0x54) above space (0x20), so deadlines always looked greater than now. The fix wraps the column in `datetime(...)` to normalise.

**Fix**: Bump fleet's `nixfleet` input past the fix, redeploy cp.

## CP stair-steps backwards through deploy history

**Symptom**: After deploying a new fleet rev, cp dispatches itself to OLDER closures on every CP restart. Each restart steps backwards.

**Cause**: Old behaviour - CP primed `verified_fleet` from the compile-time `--artifact` path, which is always the previous CI release (the `[skip ci]` release commit lands AFTER the closure is built). Each closure restart re-primed from a one-step-older artifact. The CP now does a synchronous Forgejo prime BEFORE opening the listener; per-tick re-verify is gated on `signed_at` freshness.

**Fix**: Bump fleet's `nixfleet` input past the fix, redeploy cp once. After that the cascade is permanently broken.

## Agent re-dispatches the same target every checkin (ghost loop)

**Symptom**: DB shows the same `(hostname, rollout_id, target_closure_hash)` confirmed every 60s. Activation appears successful but never settles.

**Cause**: Old behaviour - agent's `closure_hash_from_path` stripped after the first `-`, returning just the 32-char hash. CP declares the FULL basename. String comparison never equal -> `Decision::Dispatch` every checkin. The fix returns the full basename.

**Fix**: Bump fleet's `nixfleet` input past the fix, redeploy each host.

## CP's current closure ≠ artifact's declared, even when cp is on the latest deploy

**Symptom**: CP's current is `XXXXXXX-nixos-system-cp-...0810_5176864f_turbo-otter`, artifact says `YYYYYYY-..._5176864f_turbo-otter`. Same nixfleet rev suffix but different store hashes.

**Cause**: The fleet flake references `inputs.self/releases/fleet.resolved.json` for the CP's artifact path. When CI runs, it builds the closure BEFORE committing the new release artifact. An operator workstation may build AFTER, with the new artifact in the source tree. Different `inputs.self` -> different closure hash.

**Fix**: One activation cycle naturally converges (cp activates to the artifact-declared closure, which then matches on the next checkin). Not a bug; an artifact of the self-referential design. Tracked but not actively fixed - decoupling the artifact path from `inputs.self` is a possible future change.

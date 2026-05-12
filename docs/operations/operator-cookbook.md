# Operator cookbook

Tasks the operator does, with concrete commands. Add new recipes when something becomes routine.

The recipes below use these placeholders:

- `<fleet>` - your fleet repo (the one with `flake.nix` + `mkFleet`).
- `<secrets>` - your agenix secrets repo, if separate from `<fleet>`.
- `cp` - the host running `services.nixfleet-control-plane`.
- `workstation` - any host with `nixfleet.operator` enabled (where you run the `nixfleet` CLI).
- `newhost`, `stuckhost` - example host names per recipe.

Substitute your own values throughout.

## Deploy a fleet change

```sh
# 1. Edit fleet config locally
$EDITOR <fleet>/...

# 2. Commit + push to origin
git -C <fleet> commit -am "feat: ..."
git -C <fleet> push origin main

# 3. CI runs; commits a [skip ci] release commit ~minutes later
# 4. CP's channel-refs poll picks up the new artifact within 60s
# 5. Agent's next checkin: dispatch fires, agent activates, confirms

# To verify the deploy reached cp:
ssh root@cp "journalctl -u nixfleet-control-plane.service --since '5 minutes ago' \
  --no-pager | grep -E 'snapshot refreshed|dispatch|confirm received'"
```

If cp gets stuck (rare since the prime + freshness-gate fixes), redeploy directly:
```sh
nh os switch .#cp --target-host root@cp --use-remote-sudo
```

## Mint a bootstrap token for a new host

```sh
# On an operator workstation (any host with nixfleet.operator enabled)
nixfleet mint-token \
  --hostname newhost \
  --csr-pubkey-fingerprint <SHA-256-base64-of-newhost-pubkey> \
  --org-root-key /run/agenix/org-root-key \
  > newhost-token.json

# Encrypt to newhost via agenix; commit to <secrets>/agents/
agenix -e <secrets>/agents/newhost-bootstrap-token.age < newhost-token.json
git -C <secrets> commit -am "agents/newhost-bootstrap-token"
git -C <secrets> push origin main

# Bump fleet's lock; deploy newhost
nix flake update secrets -C <fleet>
git -C <fleet> commit -am "chore(flake): bump secrets for newhost"
git -C <fleet> push origin main
```

## Revoke a host's cert

```sh
# Open the SQLite DB on cp and insert a cert_revocations row.
ssh root@cp "nix-shell -p sqlite --run \
  \"sqlite3 /var/lib/nixfleet-cp/state.db <<SQL
INSERT INTO cert_revocations (hostname, not_before, reason, revoked_by)
VALUES ('newhost', datetime('now'), 'compromised', '<your-name>');
SQL\""

# Newhost's existing cert is now rejected on every /v1/* call.
# To re-enroll: mint a fresh bootstrap token + redeploy newhost.
```

## Rotate the org root key

The org root key is the trust anchor for bootstrap tokens. Rotating it means:

1. Operator generates a new ed25519 keypair on an operator workstation.
2. Encrypt the private half to the operator user(s) only via agenix -> `<secrets>/org-root-key.age`. cp MUST NOT be a recipient.
3. Update `<fleet>/.../trust.nix`:
   - Move the current `nixfleet.trust.orgRootKey.current` to `.previous` (rotation grace window).
   - Set `.current` to the new public half.
4. Commit + push fleet -> CI re-signs -> cp picks up the new trust.json on next deploy.
5. Old tokens minted under the previous key keep working for the rotation window (until the next config change moves `.previous` to null).

## Diagnose a stuck agent

```sh
ssh root@stuckhost "
  echo '=== agent status ==='
  systemctl is-active nixfleet-agent.service
  echo '=== last 50 agent log lines ==='
  journalctl -u nixfleet-agent.service -n 50 --no-pager
  echo '=== current-system ==='
  readlink /run/current-system | xargs basename
"
```

Then check what the CP saw last from this host:
```sh
ssh root@cp "nix-shell -p sqlite --run \
  \"sqlite3 /var/lib/nixfleet-cp/state.db \\
    'SELECT id, rollout_id, state, datetime(dispatched_at), datetime(confirmed_at) \
     FROM pending_confirms WHERE hostname = \\\"stuckhost\\\" ORDER BY id DESC LIMIT 5;'\""
```

Look for: rows in `pending` long after deadline (rollback timer broken), repeated dispatches for the same target (closure_hash format drift), `rolled-back` rows (deadline expired before agent activated).

## Add a host to the fleet

1. Add the host's `mkHost { ... }` call in `<fleet>/flake.nix`.
2. Mint a bootstrap token (recipe above).
3. Add the host to `<secrets>/secrets.nix` recipient lists for the secrets it should have access to.
4. `nixos-anywhere --flake .#newhost root@<bootstrap-ip>`.
5. New host enrolls on first boot (uses the bootstrap token to get an mTLS cert), checks in, gets dispatched its declared closure.

## Tag a release

```sh
# Tag a stable point - useful before major refactors so we have a known-good restore.
git -C <fleet> tag -m "v0.2.0-rc1: dispatch chain on hardware" v0.2.0-rc1
git -C <fleet> push origin v0.2.0-rc1
```

# Bootstrap-token lifecycle

Operator runbook for minting, declaring, deploying, and consuming
bootstrap tokens under the post-#96 allowlist regime.

## Minting + declaring

```
$ nixfleet mint-token \
    --hostname newhost \
    --org-root-key /run/agenix/org-root-key \
    --fleet-resolved /tmp/fleet.resolved.json \
    > /tmp/bootstrap-token-newhost.json

nonce: 1ed727e1f9c24e6ab87eb9693ba35e26
expiresAt: 2026-05-13T22:57:45Z

Add to fleet.nix `bootstrapNonces`, commit, and push:

  {
    nonce = "1ed727e1f9c24e6ab87eb9693ba35e26";
    hostname = "newhost";
    expiresAt = "2026-05-13T22:57:45Z";
    mintedAt = "2026-05-12T22:57:45Z";
    mintedBy = "ci-runner";
  }
```

Paste the snippet into your fleet repo's `fleet.nix` under
`bootstrapNonces = [ ... ];`, commit, and push.

## CI signing

Forgejo CI runs `nixfleet-release --bootstrap-nonces-attr
'fleet.bootstrapNonces' ...`, which:

1. Reads the operator-declared list from `fleet.nix`.
2. Prunes entries with `expiresAt < signedAt` (auto-audit pruning).
3. Builds `BootstrapNonces` payload + canonicalises.
4. Signs via `tpm-sign` (same trust class as `fleet.resolved.json`).
5. Writes `releases/bootstrap-nonces.json` + `.sig`.
6. Commits + pushes alongside `fleet.resolved.json`.

Typical CI cycle: ~2 min.

## CP applies the allowlist

CP polls `bootstrap-nonces.json` every 60 s. On a successful
verify, it replaces the in-memory `AllowedNoncesView` wholesale.

The CP refuses to serve `/v1/*` requests until at least one
verified allowlist has been applied
(`bootstrap_nonces_primed = true`).

## Deploying the token to the host

```
# scp to host's /tmp
$ scp /tmp/bootstrap-token-newhost.json newhost:/tmp/

# install root-only on the host
$ ssh root@newhost '
    install -m 400 -o root -g root \
      /tmp/bootstrap-token-newhost.json \
      /var/lib/nixfleet/bootstrap-token-newhost.json
    shred -u /tmp/bootstrap-token-newhost.json
'
```

The agent must have `--bootstrap-token-file
/var/lib/nixfleet/bootstrap-token-newhost.json` in its unit cmdline.
Set this via the NixOS option
`services.nixfleet-agent.bootstrapTokenFile` in your fleet config
and let the next rebuild propagate.

## Triggering enrolment

```
$ ssh root@newhost '
    rm /var/lib/nixfleet/agent-cert.pem
    systemctl restart nixfleet-agent
'
```

The agent enters first-boot enrolment, reads the bootstrap token,
posts to `/v1/enroll`. The CP verifies the token signature, looks
up the nonce in the allowlist, and issues a 10-min cert (or
whatever `agentCertValiditySecs` is set to).

## Post-enrolment

The nonce is consumed:
- In the signed allowlist: it stays until the operator removes
  it OR until `expiresAt` passes and the next CI sign cycle
  prunes it.
- In CP state.db (`enroll_token_nonces`): replays within the
  current CP DB lifecycle return 409.

If the CP DB is wiped: the signed allowlist still has the entry
until pruned by expiry, so a replay would return either:
- **401 `nonce_allowlist_expired`** if the allowlist's
  `expiresAt` has passed (the operator's lever)
- **200 OK with JSON `EnrollResponse` body** (new cert issued) if the allowlist's `expiresAt`
  is still in the future AND the operator hasn't removed the
  entry - this is the small replay window that exists by
  design until the operator manages the allowlist (or until
  `expiresAt` passes naturally).

To narrow this window: keep token validity short (default 24h),
or remove the entry from `fleet.nix` after enrolment confirms
to commit + sign.

## Disaster recovery

If `state.db` is wiped (Refinery checksum mismatch, disk loss,
intentional rebuild):

1. CP starts up clean.
2. Pollers run; `bootstrap-nonces.json` applied to memory.
3. CP can re-issue certs to hosts whose nonces are still in the
   allowlist (and whose tokens are still on disk).
4. For hosts whose nonces have been removed/expired: operator
   re-mints + re-declares.

No host is "permanently dead" from a CP rebuild - full
re-enrolment is always available given operator action.

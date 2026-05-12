# Disaster recovery - destroying the control plane

Background: see `../design/architecture.md` §6 (CP-resident state by recovery profile) + §8.

Operator runbook for wiping the CP and rebuilding from signed artifacts.
Validation: `fleet-harness-teardown` in CI.

## Pre-flight

Before destroying state, confirm:

1. Signed artifacts reachable (`fleet.resolved.json` + `.sig`, and
   `revocations.json` + `.sig` if configured) from the URLs in
   `--channel-refs-artifact-url` / `--revocations-artifact-url`.
2. Build-time fallback intact (`--artifact` / `--signature` /
   `--trust-file`).
3. Fleet CA available (`--fleet-ca-cert` / `--fleet-ca-key`). Required
   only for `/v1/enroll` + `/v1/agent/renew`; existing agents keep
   working without it.
4. At least one agent currently online.

If any check fails, fix the prerequisite first - do not proceed.

## Procedure

```bash
# 1. Stop the service.
systemctl stop nixfleet-control-plane.service

# 2. Wipe the SQLite database (leave audit.log if present).
rm -rf /var/lib/nixfleet-cp/state.db \
       /var/lib/nixfleet-cp/state.db-wal \
       /var/lib/nixfleet-cp/state.db-shm

# 3. Restart.
systemctl start nixfleet-control-plane.service
```

> **Do not** delete `/etc/nixfleet/cp/trust.json`,
> `/etc/nixfleet/cp/fleet-ca-*.pem`, or anything under `/etc/nixfleet/cp/`.
> Those are flake-provided trust roots; deleting them turns recovery
> from "outage" into "breach".

CP restart reopens a fresh DB, reads `trust.json`, primes
`verified_fleet` (upstream poll first, build-time fallback), replays the
signed revocations sidecar if configured, and resumes accepting checkins.
With production 60s polling, expect full agent repopulation in 70-120s.

## Verify

```bash
# CP healthy, snapshot primed (within ~30s of restart).
curl -sk https://localhost:8443/healthz | jq '.last_tick_at != null'

# Verified-fleet snapshot is fresh (mTLS - substitute your operator pair).
curl -sk \
  --cacert /etc/nixfleet/cp/ca.pem \
  --cert <CLIENT_CERT_PEM> --key <CLIENT_KEY_PEM> \
  https://localhost:8443/v1/channels/stable | jq '.signed_at'

# Revocations sidecar replayed (when configured).
journalctl -u nixfleet-control-plane.service --since='5 min ago' \
  | grep -E 'revocations poll|cert_revocations'

# Every expected agent has checked in.
journalctl -u nixfleet-control-plane.service --since='5 min ago' \
  | grep 'checkin received' | awk '{print $NF}' | sort -u
```

All four pass -> recovery is complete.

## When this fails

- **CP refuses to start.** Check the verify-fleet error: `--trust-file`
  permissions, corrupted build-time artifact (roll back the flake
  commit), or unexpected schema state on a non-empty DB (file a bug if
  you wiped per Step 2).
- **Agents don't reconnect.** `journalctl -u nixfleet-agent.service`
  on the host - usually cert expiry or revocation. Re-enroll via the
  bootstrap-token flow.
- **Recovery > 10× target.** Upstream-fetch issue: Forgejo down, expired
  auth token, network partition. `journalctl -u
  nixfleet-control-plane.service | grep channel-refs`.

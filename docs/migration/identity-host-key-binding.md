# Migration: agent identity bound to SSH host key (closes #43, #9)

Closes RFC-0003 §2 drift: the agent's mTLS cert is now bound to the
host's SSH host key (`/etc/ssh/ssh_host_ed25519_key`), not a fresh
keypair. The CP refuses any enrollment or renewal whose CSR pubkey
doesn't match the host's declared `nixfleet.fleetSchema.hosts.<hostname>.pubkey`.

## What changed

| Layer | Before | After |
|---|---|---|
| Agent enrollment CSR | rcgen-generated fresh ed25519 keypair | OpenSSH host key, decoded → PKCS#8 PEM → `KeyPair::from_pem` |
| Agent renewal CSR | Fresh keypair every 30 days | Same SSH host key — pubkey unchanged across renewals |
| `--client-key` flag | Pointed at agenix-deployed `agents/<host>-key.age` | Defaults to `/etc/ssh/ssh_host_ed25519_key`; agenix path no longer needed |
| `--client-cert` flag | Pointed at agenix-deployed `agents/<host>-cert.age` (tmpfs) | Defaults to `/var/lib/nixfleet/agent-cert.pem` — writable + persistent under `nixfleet.persistence.directories` |
| CP `/v1/enroll` | Validated bootstrap token + nonce + CN/fingerprint | Above PLUS: CSR pubkey must equal `hosts.<hostname>.pubkey` from verified fleet snapshot |
| CP `/v1/agent/renew` | Validated mTLS, signed cert | Above PLUS: same fleet-pubkey binding check |
| Soak attestation (`last_confirmed_at`) | Trusted on its face | Signed by SSH host key; CP verifies before applying. Unsigned attestations silently ignored. |

## Operator runbook

Before deploying the new agent code, declare every existing host's
SSH host pubkey in `fleet.nix`. Without this, agents fail to renew
once their existing 30-day cert expires.

### Step 1 — collect SSH host pubkeys

For each fleet host (krach, lab, ohm, pixel, aether, …):

```sh
ssh root@<host> 'cat /etc/ssh/ssh_host_ed25519_key.pub' | tr -d '\n'
```

Copy the entire `ssh-ed25519 AAAA… root@<host>` line.

### Step 2 — declare in fleet.nix

```nix
nixfleet.fleetSchema.hosts.krach = {
  system = "x86_64-linux";
  channel = "stable";
  pubkey = "ssh-ed25519 AAAAC3Nz...";  # paste from step 1
  # …
};
```

Repeat for every host. Push to lab; CI signs a new `fleet.resolved.json`
carrying every host's `pubkey`.

### Step 3 — deploy the new framework

Bump `fleet/flake.lock`'s `nixfleet` input to a release that includes
this PR. Push to lab; CI signs a new release. Wait for `system.autoUpgrade`
to roll the new closure to workstations (~90 min via hourly+jitter).

### Step 4 — verify renewal cycle

Within 30 days, every agent's existing cert expires and triggers
`/v1/agent/renew`. The renewal CSR is signed by the SSH host key;
the CP validates against the declared pubkey; new cert lands.

Watch `journalctl -u nixfleet-control-plane` on lab during the
window. Look for either:

- `target: "issuance" hostname=<host> not_after=<...> "renewed"` — success
- `enroll: fleet-pubkey binding check failed` — declared pubkey mismatch (Step 1 incorrect)
- `renew: host not declared in fleet.nix` — declaration missing (Step 2 incomplete)

### Step 5 — drop redundant agenix entries

Once every host has rotated to a host-key-bound cert (verified in
Step 4), the per-host private-key agenix entries become unused.
Drop from `~/dev/abstracts33d/fleet-secrets/secrets.nix`:

- `"agents/krach-key.age".publicKeys = …`
- `"agents/lab-key.age".publicKeys = …`
- `"agents/ohm-key.age".publicKeys = …`
- `"agents/pixel-key.age".publicKeys = …`
- `"agents/aether-key.age".publicKeys = …`

And delete the corresponding `.age` files. The `<host>-cert.age`
entries can stay (the issued cert is still agenix-deployed; the
private key half is what's redundant), but the agent now writes the
cert from the enrollment/renewal response itself, so those become
purely defensive — operator can clean them up in a follow-up cycle.

## New host onboarding (closes #9 fully)

For a fresh host added to the fleet:

```sh
# 1. nixos-anywhere installs the host with a fresh SSH host key.
nixos-anywhere --flake .#newhost root@newhost.lan

# 2. Read the host's pubkey from the install.
ssh root@newhost.lan 'cat /etc/ssh/ssh_host_ed25519_key.pub'

# 3. Declare in fleet.nix:
#      hosts.newhost.pubkey = "ssh-ed25519 ...";

# 4. CI signs new fleet.resolved.json.

# 5. Mint a bootstrap token scoped to the declared pubkey:
nixfleet mint-token \
    --hostname newhost \
    --fleet-resolved /var/lib/nixfleet/releases/fleet.resolved.json \
    --org-root-key /run/secrets/org-root-key

# 6. Place the token at /var/lib/nixfleet/bootstrap-token.json on
#    the new host. Agent enrols on next start.
```

The `--fleet-resolved` flag derives the fingerprint from the declared
pubkey — no manual SHA-256 dance, no token-vs-host-key drift.

## Compatibility notes

- Existing certs (bound to fresh keypairs) keep working until expiry.
  No flag day; migration is per-host at next renewal.
- Hosts whose `hosts.<host>.pubkey` is missing get `UNAUTHORIZED` on
  enrollment AND renewal — fail-closed by design.
- Lab CP itself is enrolled via agenix-deployed cert, not `/v1/enroll`,
  so it's unaffected by this change.

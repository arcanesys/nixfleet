# nixfleet-trust-bootstrap

Operator-side tool that mints the fleet's offline root CA and signs the
TPM-bound issuance CA cert. Run once when standing up a new fleet, then
again at issuance-CA renewal (default 1y), TPM rotation, or disaster
recovery.

## Usage

On the operator workstation:

```sh
nix run .#nixfleet-trust-bootstrap -- \
  --output-dir ~/.config/nixfleet \
  --cp-host cp
```

Outputs (under `--output-dir`):

| File | What | Where it goes |
|---|---|---|
| `fleet-root.cert.pem` | Fleet root CA cert (public) | trust.json via `nixfleet.trust.rootCAPem` |
| `fleet-root.key.pem` | Fleet root CA private key | **OFFLINE** on the operator workstation, mode 0600 |
| `fleet-issuance-ca.cert.pem` | Issuance CA cert (public, TPM-bound) | scp to cp as `/etc/nixfleet/cp/issuance-ca.pem` |
| `trust-snippet.json` | trust.json fragment | merge into fleet config |

## Standing function

The script supports four standing operator workflows:

1. **New fleet stand-up** - generate root + sign issuance CA from a
   freshly-provisioned TPM keyslot. Run once per fleet.
2. **Issuance CA renewal** - resign the issuance CA cert (default
   validity 1y) using the same TPM pubkey. Run yearly.
3. **TPM rotation** - sign a new issuance CA from a new TPM keyslot
   after hardware replacement. Same root, new TPM-bound CA.
4. **Disaster recovery** - rebuild cp CP from scratch; the offline
   root signs whatever new TPM is provisioned. Same root identity, new
   issuance chain.

## Prerequisites

The cp CP host must have run the `nixfleet-tpm-keyslot-provision-<name>`
systemd service, leaving the TPM-bound public key at
`/var/lib/nixfleet-tpm-keyslot/<name>/pubkey.raw`. The bootstrap script
SSHes the cp host and reads that file.

Default keyslot name is `issuanceCA`; override with
`--tpm-keyslot-name`.

## Output dir convention

The script writes to `${HOME}/.config/nixfleet` by default. The fleet
root key (`fleet-root.key.pem`, mode 0600) stays there permanently  - 
the operator's workstation IS the offline root custody location until
a Yubikey migration lands.

The other artifacts (cert, issuance CA cert, trust snippet) are
durable reference material; keep them alongside the key.

# Bundle C migration runbook (#41)

End-to-end migration from the legacy file-backed issuance CA (`fleet-ca-key.age` in agenix) to the TPM-bound issuance CA. Walk through these in order on a single CP host (lab today).

## Prerequisites

- Framework on lab is at C.4 or later (C.1 keyslots substrate, C.2 `CaSigner` trait, C.3 trust-chain schema, C.4 `nixfleet-cp-bootstrap` available).
- Operator workstation has the bootstrap script: `nix run github:abstracts33d/nixfleet#nixfleet-cp-bootstrap -- --help`.
- D12: file-based root for v1; Yubikey path tracked separately.

## Step 1 — Declare the issuance CA keyslot

Add to fleet's coordinator scope (or directly to lab's config):

```nix
# fleet/modules/scopes/coordinator/default.nix (or per-host)
nixfleet.keyslots.tpm.keys.issuanceCA = {
  handle = "0x81010002";          # distinct from ciReleaseKey at 0x81010001
  algorithm = "ecdsa-p256";
  pcrPolicy = ["0"];              # D16: PCR 0 (UEFI firmware) for C.1
};
```

Push, wait for CI to publish, lab converges. Verify:

```sh
ssh lab 'systemctl status nixfleet-tpm-keyslot-provision-issuanceCA'
ssh lab 'wc -c /var/lib/nixfleet-tpm-keyslot/issuanceCA/pubkey.raw'   # must be 64
```

## Step 2 — Mint the offline root + issuance CA cert

On the operator workstation, run the bootstrap script:

```sh
nix run .#nixfleet-cp-bootstrap -- \
  --output-dir ~/.config/nixfleet/bundle-c \
  --lab-host lab
```

The script:
- Generates `fleet-root.{key,cert}.pem` (10y, ECDSA P-256). **Keep `fleet-root.key.pem` offline.**
- SSHes lab, reads the TPM pubkey.
- Mints `fleet-issuance-ca.cert.pem` signed by the root, with the locked D14 extensions (BC critical pathlen:0, EKU clientAuth, name constraints `DNS:fleet.lab.internal`).
- Writes `trust-snippet.json` with `rootCAPem` + `issuanceCAPems[]` ready to merge.
- Writes `README.txt` with the follow-up steps.

## Step 3 — Ship the issuance CA cert to lab

```sh
scp ~/.config/nixfleet/bundle-c/fleet-issuance-ca.cert.pem \
    lab:/etc/nixfleet/cp/issuance-ca.pem
```

This file is intentionally **not** in fleet-secrets — it's a public cert, not a secret.

## Step 4 — Update fleet config

In fleet's `modules/nixfleet/trust.nix` (or the equivalent host-level wiring):

```nix
nixfleet.trust = {
  # … existing fields …
  rootCAPem = builtins.readFile ./bundle-c/fleet-root.cert.pem;
  issuanceCAPems = [
    (builtins.readFile ./bundle-c/fleet-issuance-ca.cert.pem)
    # During the overlap window, also include the legacy single-tier
    # CA so old agent certs (signed by fleet-ca-key.age) still chain
    # through trust.json:
    (builtins.readFile /etc/nixfleet/fleet-ca.pem)
  ];
};
```

Switch the CP daemon flags (in fleet's CP module, typically `modules/infra/control-plane.nix` or wherever `services.nixfleet-control-plane` is configured):

```nix
services.nixfleet-control-plane.extraArgs = [
  # NEW (TPM-backed signer):
  "--tpm-ca-pubkey-raw" "/var/lib/nixfleet-tpm-keyslot/issuanceCA/pubkey.raw"
  "--tpm-ca-sign-wrapper" "/run/current-system/sw/bin/tpm-sign-issuanceCA"
  "--fleet-ca-cert" "/etc/nixfleet/cp/issuance-ca.pem"

  # KEEP for the overlap window (#41 §"Migration when this issue lands"):
  # "--fleet-ca-key" "/run/agenix/fleet-ca-key"
];
```

The CP's `state::ServeArgs` handler picks `TpmCaSigner` when both TPM flags are present + `--fleet-ca-cert` is set; falls back to `FileCaSigner` otherwise. With the legacy `--fleet-ca-key` flag still present, you have a one-flag rollback if anything misbehaves.

## Step 5 — Push, converge, verify

```sh
git -C abstracts33d/fleet push lab main
# wait for CI to publish
ssh lab 'readlink /run/current-system'    # should hop to a closure with the new flags
ssh lab 'journalctl -u nixfleet-control-plane -n 30 | grep "issuance CA signer"'
# expected: "issuance CA signer: TPM-backed"
```

If the log says `file-backed` instead, the TPM flags didn't take effect — recheck the CP module wiring.

## Step 6 — Trigger one renewal cycle

Restart each agent so it hits `/v1/agent/renew` against the new TPM-backed issuance:

```sh
for h in krach aether ohm pixel; do
  ssh "$h" 'sudo systemctl restart nixfleet-agent'
done
```

Verify the issued cert is signed by the new issuance CA:

```sh
ssh krach 'openssl x509 -in /var/lib/nixfleet/agent-cert.pem -noout -issuer -ext extendedKeyUsage'
# expected issuer: CN=Fleet Issuance CA
# expected EKU:    TLS Web Client Authentication
```

Verify CN was canonicalised (C.3):

```sh
ssh krach 'openssl x509 -in /var/lib/nixfleet/agent-cert.pem -noout -subject'
# expected: subject=CN=agent-krach.fleet.lab.internal
```

Verify chain validates against the new root:

```sh
ssh krach '
  cat /var/lib/nixfleet/agent-cert.pem /etc/nixfleet/cp/issuance-ca.pem > /tmp/chain.pem
  openssl verify -CAfile /etc/nixfleet/fleet-root.pem /tmp/chain.pem
'
# expected: /tmp/chain.pem: OK
```

## Step 7 — Watch the overlap window

For the next 30 days (or however long you want the overlap), both:
- agents holding pre-C.3 certs (CN = bare machineId, signed by file-backed CA), AND
- agents holding post-C.3 certs (CN = canonical FQDN, signed by TPM-backed issuance CA)

work simultaneously. The CP's mTLS `--client-ca` includes both CA certs (concat); `extract_machine_id` handles either CN format.

`/v1/agent/renew` reissues certs in the new format on every renewal. After all agents have rotated through one renewal (≤30 days for AGENT_CERT_VALIDITY at 30 days), every cert is post-C.3.

## Step 8 — Drop the legacy file-backed CA

Once you've confirmed every agent is on a post-C.3 cert (audit log + per-host inspection):

1. Remove `--fleet-ca-key` from CP daemon flags.
2. Remove the legacy `fleet-ca.pem` from `nixfleet.trust.issuanceCAPems`.
3. Remove the legacy `fleet-ca.pem` from CP's `--client-ca` (if it was being used as a trust root).
4. Decrypt + delete `fleet-ca-key.age` from fleet-secrets:
   ```sh
   git -C abstracts33d/fleet-secrets rm secrets/fleet-ca-key.age
   git commit -m "drop fleet-ca-key — replaced by TPM-bound issuance CA (closes #41)"
   ```
5. Push, lab converges with the legacy path fully removed.

The CP daemon log on the next boot should still say `issuance CA signer: TPM-backed` — and now there's no fallback path. Any `enroll` or `renew` failure is a pure-TPM failure (TPM unavailable, PCR mismatch, etc.) — easier to diagnose without the legacy fallback masking it.

## Rollback

At any point during the overlap window (Steps 4–7), reverting the CP module commit restores file-backed signing:

```sh
git -C abstracts33d/fleet revert <bundle-c-flag-commit>
git push lab main
```

Lab converges back to the previous closure; CP restarts on `FileCaSigner`. Agent certs already issued by the TPM-backed signer remain valid (their chain is in trust.json), but new issuances go back through `--fleet-ca-key`.

After Step 8 (`fleet-ca-key.age` deleted), rollback requires re-deriving the legacy CA from a backup. Don't take Step 8 until you're confident.

## Verification checklist

- [ ] Step 1: TPM keyslot provisioned, pubkey.raw is 64 bytes
- [ ] Step 2: bootstrap output exists, root key is mode 0600
- [ ] Step 3: `/etc/nixfleet/cp/issuance-ca.pem` on lab, `openssl verify` clean
- [ ] Step 4: fleet config committed with new trust.json fields + CP flags
- [ ] Step 5: CP log shows `issuance CA signer: TPM-backed`
- [ ] Step 6: every agent reissued, CN is canonical FQDN, chain validates
- [ ] Step 7: 30-day overlap window observed clean (no auth failures in CP audit log)
- [ ] Step 8: `fleet-ca-key.age` deleted, `--fleet-ca-key` flag removed

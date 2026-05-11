#!/usr/bin/env bash
# nixfleet-trust-bootstrap — operator tool that mints the offline fleet
# root CA and signs the TPM-bound issuance CA cert.
#
# Standing workflows: new-fleet stand-up, annual issuance-CA renewal,
# TPM rotation, disaster recovery.
#
# Prerequisite: lab has converged onto a closure that declares the
# TPM keyslot in `nixfleet.keyslots.tpm.keys.<name>`, and the
# `nixfleet-tpm-keyslot-provision-<name>` systemd service has run
# successfully (pubkey.raw exists at
# /var/lib/nixfleet-tpm-keyslot/<name>/pubkey.raw).

set -euo pipefail

# ── defaults ─────────────────────────────────────────────────────────
lab_host="lab"
tpm_keyslot_name="issuanceCA"
tpm_pubkey_path="" # derived from keyslot name unless overridden
agent_cn_suffix="fleet.lab.internal"
output_dir="${HOME}/.config/nixfleet"
root_key=""
root_validity_days=3650        # ~10y
intermediate_validity_days=365 # 1y
dry_run=0
assume_yes=0

usage() {
  cat <<EOF >&2
Usage: nixfleet-trust-bootstrap [OPTIONS]

Generate the offline fleet root CA + sign the TPM-bound issuance CA cert.

OPTIONS:
  --lab-host <host>            SSH target for the CP host (default: lab)
  --tpm-keyslot-name <name>    Keyslot name on lab (default: issuanceCA)
  --tpm-pubkey-path <path>     Override TPM pubkey path on lab
                               (default: /var/lib/nixfleet-tpm-keyslot/<name>/pubkey.raw)
  --agent-cn-suffix <fqdn>     Name constraint domain (default: fleet.lab.internal)
  --output-dir <dir>           Where to write artefacts (default: ~/.config/nixfleet)
  --root-key <path>            Use existing root key file (skip generation;
                               cert auto-derived if <key>.cert.pem missing)
  --root-validity-days <n>     Self-signed root validity (default: ${root_validity_days})
  --intermediate-validity-days <n>  Issuance CA validity (default: ${intermediate_validity_days})
  --dry-run                    Print steps without executing
  --yes                        Skip confirmation prompts
  -h, --help                   This help

OUTPUT (in <output-dir>):
  fleet-root.cert.pem          Fleet root CA cert (publish in trust.json)
  fleet-root.key.pem           Fleet root CA private key (KEEP OFFLINE)
  fleet-issuance-ca.cert.pem   Issuance CA cert (ship to lab as
                               /etc/nixfleet/cp/issuance-ca.pem)
  trust-snippet.json           trust.json fragment to merge into fleet config
  README.txt                   Operator follow-up steps
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

# ── arg parse ────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
  --lab-host)
    lab_host=$2
    shift 2
    ;;
  --tpm-keyslot-name)
    tpm_keyslot_name=$2
    shift 2
    ;;
  --tpm-pubkey-path)
    tpm_pubkey_path=$2
    shift 2
    ;;
  --agent-cn-suffix)
    agent_cn_suffix=$2
    shift 2
    ;;
  --output-dir)
    output_dir=$2
    shift 2
    ;;
  --root-key)
    root_key=$2
    shift 2
    ;;
  --root-validity-days)
    root_validity_days=$2
    shift 2
    ;;
  --intermediate-validity-days)
    intermediate_validity_days=$2
    shift 2
    ;;
  --dry-run)
    dry_run=1
    shift
    ;;
  --yes)
    assume_yes=1
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    usage
    die "unknown arg: $1"
    ;;
  esac
done

[[ -n $output_dir ]] || die "internal: --output-dir empty after default applied"
if [[ -z $tpm_pubkey_path ]]; then
  tpm_pubkey_path="/var/lib/nixfleet-tpm-keyslot/${tpm_keyslot_name}/pubkey.raw"
fi

run() {
  if [[ $dry_run == "1" ]]; then
    printf '+ %s\n' "$*" >&2
  else
    "$@"
  fi
}

confirm() {
  [[ $assume_yes == "1" ]] && return 0
  read -r -p "$1 [y/N] " ans
  [[ $ans =~ ^[Yy]$ ]]
}

# ── pre-flight checks ────────────────────────────────────────────────
echo "▸ pre-flight"
for t in openssl ssh jq; do
  command -v "$t" >/dev/null || die "$t not found in PATH"
done

ssh -o BatchMode=yes -o ConnectTimeout=10 "$lab_host" true ||
  die "ssh $lab_host failed — check connectivity / authorized_keys"

# shellcheck disable=SC2029
# $tpm_pubkey_path expansion-on-client is intended — it's an operator-
# supplied path interpreted on the operator workstation.
ssh "$lab_host" test -f "$tpm_pubkey_path" ||
  die "TPM keyslot pubkey not found at $tpm_pubkey_path on $lab_host.
  Has lab converged onto a closure with
  nixfleet.keyslots.tpm.keys.${tpm_keyslot_name} configured?
  Has the nixfleet-tpm-keyslot-provision-${tpm_keyslot_name}
  systemd service run successfully?"

mkdir -p "$output_dir"

# ── 1. fleet root CA ─────────────────────────────────────────────────
root_key_pem="$output_dir/fleet-root.key.pem"
root_cert_pem="$output_dir/fleet-root.cert.pem"

if [[ -n $root_key ]]; then
  echo "▸ using existing root key: $root_key"
  cp "$root_key" "$root_key_pem"
  chmod 0600 "$root_key_pem"
  # If a sibling .cert.pem exists, copy it; else self-sign a new cert.
  src_cert="${root_key%.key.pem}.cert.pem"
  if [[ -f $src_cert ]]; then
    cp "$src_cert" "$root_cert_pem"
  else
    echo "▸ no sibling cert found; self-signing new root cert (${root_validity_days}d)"
    run openssl req -new -x509 -days "$root_validity_days" \
      -key "$root_key_pem" \
      -subj "/CN=Fleet Root CA/O=arcanesys/OU=fleet" \
      -out "$root_cert_pem"
  fi
else
  if [[ -f $root_key_pem ]]; then
    confirm "$root_key_pem already exists — overwrite?" ||
      die "aborted by user"
  fi
  echo "▸ generating fleet root CA (ECDSA P-256, ${root_validity_days}d)"
  run openssl ecparam -name prime256v1 -genkey -noout -out "$root_key_pem"
  run chmod 0600 "$root_key_pem"
  run openssl req -new -x509 -days "$root_validity_days" \
    -key "$root_key_pem" \
    -subj "/CN=Fleet Root CA/O=arcanesys/OU=fleet" \
    -out "$root_cert_pem"
fi

# ── 2. read TPM pubkey ───────────────────────────────────────────────
echo "▸ reading TPM pubkey from $lab_host:$tpm_pubkey_path"
tmp_pubkey_raw=$(mktemp)
tmp_pubkey_pem=$(mktemp --suffix=.pem)
tmp_throwaway_key=$(mktemp --suffix=.key)
tmp_csr=$(mktemp --suffix=.csr)
tmp_ext=$(mktemp --suffix=.cnf)
cleanup() {
  rm -f "$tmp_pubkey_raw" "$tmp_pubkey_pem" \
    "$tmp_throwaway_key" "$tmp_csr" "$tmp_ext"
}
trap cleanup EXIT

# shellcheck disable=SC2029
# Same as above — operator-supplied path, intended local expansion.
ssh "$lab_host" cat "$tpm_pubkey_path" >"$tmp_pubkey_raw"

# Verify length: P-256 raw point is 64 bytes (X || Y, no 0x04 prefix per
# the keyslots scope contract).
size=$(wc -c <"$tmp_pubkey_raw")
[[ $size -eq 64 ]] ||
  die "TPM pubkey at $tpm_pubkey_path is not 64 bytes (got $size)"

# ── 3. raw → SPKI PEM ────────────────────────────────────────────────
# Prepend the DER SPKI header for ECDSA P-256 + uncompressed point
# (0x04 prefix). This is the fixed prefix every P-256 SPKI starts with;
# only the X || Y bytes vary per key.
{
  printf '\x30\x59\x30\x13\x06\x07\x2a\x86\x48\xce\x3d\x02\x01\x06\x08\x2a\x86\x48\xce\x3d\x03\x01\x07\x03\x42\x00\x04'
  cat "$tmp_pubkey_raw"
} | openssl pkey -pubin -inform DER -pubout -out "$tmp_pubkey_pem"

# ── 4. issuance CA extensions config ─────────────────────────────────
# D14 locked: name constraint = dNSName:fleet.lab.internal (and any
# subdomain), EKU = clientAuth ONLY. With pathlen:0, the issuance CA
# can mint end-entity certs but not further intermediates.
cat >"$tmp_ext" <<EOF
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, digitalSignature, keyCertSign, cRLSign
extendedKeyUsage = clientAuth
nameConstraints = critical, permitted;DNS:${agent_cn_suffix}
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
EOF

# ── 5. throwaway CSR (we replace pubkey via -force_pubkey) ───────────
echo "▸ building throwaway CSR (pubkey replaced at sign time)"
run openssl ecparam -name prime256v1 -genkey -noout -out "$tmp_throwaway_key"
run openssl req -new -key "$tmp_throwaway_key" \
  -subj "/CN=Fleet Issuance CA/O=arcanesys/OU=fleet" \
  -out "$tmp_csr"

# ── 6. sign with fleet root, swapping in TPM pubkey ──────────────────
issuance_cert="$output_dir/fleet-issuance-ca.cert.pem"
echo "▸ signing issuance CA (TPM pubkey, ${intermediate_validity_days}d)"
run openssl x509 -req \
  -in "$tmp_csr" \
  -CA "$root_cert_pem" \
  -CAkey "$root_key_pem" \
  -force_pubkey "$tmp_pubkey_pem" \
  -days "$intermediate_validity_days" \
  -extfile "$tmp_ext" \
  -CAcreateserial \
  -out "$issuance_cert"

# ── 7. trust-snippet.json fragment ───────────────────────────────────
trust_snippet="$output_dir/trust-snippet.json"
jq -n \
  --rawfile root "$root_cert_pem" \
  --rawfile issuance "$issuance_cert" \
  '{rootCAPem: $root, issuanceCAPems: [$issuance]}' \
  >"$trust_snippet"

# ── 8. operator README ───────────────────────────────────────────────
cat >"$output_dir/README.txt" <<EOF
nixfleet-trust-bootstrap output — generated $(date -u +'%Y-%m-%dT%H:%M:%SZ')

CONTENT:
  fleet-root.cert.pem        — root CA cert (publish via trust.json)
  fleet-root.key.pem         — root CA private key (KEEP OFFLINE; mode 0600)
  fleet-issuance-ca.cert.pem — issuance CA cert (ship to lab)
  trust-snippet.json         — trust.json fragment to merge into fleet config

NEXT STEPS (operator):

1. Ship the issuance CA cert to lab:
     scp $issuance_cert ${lab_host}:/etc/nixfleet/cp/issuance-ca.pem

2. Update fleet config (modules/nixfleet/trust.nix or equivalent):
     nixfleet.trust.rootCAPem = builtins.readFile <fleet-root.cert.pem>;
     nixfleet.trust.issuanceCAPems = [
       (builtins.readFile <fleet-issuance-ca.cert.pem>)
     ];

3. Switch CP daemon flags to TPM-backed signer:
     --tpm-ca-pubkey-raw $tpm_pubkey_path
     --tpm-ca-sign-wrapper /run/current-system/sw/bin/tpm-sign-${tpm_keyslot_name}
     --fleet-ca-cert /etc/nixfleet/cp/issuance-ca.pem

4. Commit, push, lab converges. Verify:
     ssh ${lab_host} 'sudo journalctl -u nixfleet-control-plane -n 20'
   should log: "issuance CA signer: TPM-backed".

5. Trigger one renewal cycle to confirm end-to-end:
     ssh <agent-host> 'sudo systemctl restart nixfleet-agent'
   then check the agent's cert was reissued by the new chain.

KEY CUSTODY:
  fleet-root.key.pem must NOT be committed to any repo. Keep on the
  operator workstation under \${HOME}/.config/nixfleet/ with mode
  0600, or migrate to Yubikey PIV slot 9c when hardware arrives.
EOF

chmod 0644 "$root_cert_pem" "$issuance_cert" "$trust_snippet" "$output_dir/README.txt"

# ── summary ──────────────────────────────────────────────────────────
echo
echo "════════════ Bootstrap complete ════════════"
echo
echo "Files written to $output_dir:"
ls -l "$output_dir"
echo
echo "Read $output_dir/README.txt for operator next steps."

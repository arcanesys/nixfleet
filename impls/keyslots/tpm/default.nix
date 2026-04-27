# TPM-backed signing keyslots. First-boot oneshot per named keyslot creates
# a primary, evicts to a persistent handle, exports the pubkey. Idempotent
# across impermanence wipes - re-extracts from the persisted handle.
#
# Multi-keyslot (cfg.keys.<name>) coexist on one TPM at distinct handles.
# Legacy singleton (top-level cfg.handle / algorithm) persists for
# backward compat - synthesised as `cfg.keys.legacy` with pubkey at
# the top of `cfg.exportPubkeyDir`, not under a per-name subdirectory.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.nixfleet.keyslots.tpm;

  # Synthesise the legacy singleton as `keys.legacy`. Always present when
  # the scope is enabled (handle has a default), preserving pre-multi-
  # keyslot behaviour for existing deployments. Layout differs: legacy
  # writes pubkey.{pem,raw} directly under exportPubkeyDir; named keys
  # write under <name>/.
  legacyKey = {
    legacy = {
      inherit (cfg) handle algorithm signWrapperName;
      # Legacy never had PCR policy support; default to userwithauth
      # (empty pcrPolicy) so existing deployments don't suddenly need
      # re-provisioning. Operators opt-in by migrating to cfg.keys.
      pcrPolicy = [];
    };
  };
  effectiveKeys = cfg.keys // legacyKey;

  algoSpec = algorithm:
    {
      "ecdsa-p256" = {
        createPrimaryArgs = "--key-algorithm ecc256:ecdsasha256";
        # LOADBEARING: DER SPKI for prime256v1 ends with 0x04 || X || Y;
        # tail 64 bytes = X || Y (CONTRACTS §II #1 raw encoding).
        extractRawCmd = pemPath: rawPath: ''
          openssl ec -pubin -in ${pemPath} -pubout -outform DER \
            | tail -c 64 > ${rawPath}
        '';
        tpmSignHashArg = "-g sha256";
        # ECDSA P-256/SHA-256: TPMT_SIGNATURE 72 bytes, R at 6..38, S at 40..72.
        extractRawSig = ''
          ${pkgs.coreutils}/bin/dd if="$1" bs=1 skip=6 count=32 status=none
          ${pkgs.coreutils}/bin/dd if="$1" bs=1 skip=40 count=32 status=none
        '';
      };
      "ed25519" = {
        createPrimaryArgs = "--key-algorithm ed25519";
        extractRawCmd = pemPath: rawPath: ''
          openssl pkey -pubin -in ${pemPath} -outform DER | tail -c 32 > ${rawPath}
        '';
        tpmSignHashArg = "-g sha256";
        # ed25519 signature layout differs from ECDSA - caller must adapt.
        extractRawSig = ''
          ${pkgs.coreutils}/bin/dd if="$1" bs=1 skip=6 count=64 status=none
        '';
      };
    }
    .${
      algorithm
    };

  # Per-key directory layout: legacy at top of exportPubkeyDir (back-compat),
  # named keys at exportPubkeyDir/<name>/.
  pubkeyDirOf = name:
    if name == "legacy"
    then cfg.exportPubkeyDir
    else "${cfg.exportPubkeyDir}/${name}";

  # Build a per-key sign wrapper. PCR-policy keys use a policy auth session;
  # userwithauth keys (legacy) use the simpler direct-sign path.
  mkSignWrapper = name: keyCfg: let
    spec = algoSpec keyCfg.algorithm;
    pcrList = lib.concatStringsSep "," (map (i: "${i}") keyCfg.pcrPolicy);
    pcrSpec = "sha256:${pcrList}";
    extractRawSigScript = pkgs.writeShellScript "tpm-extract-raw-sig-${name}" ''
      set -euo pipefail
      ${spec.extractRawSig}
    '';
  in
    pkgs.writeShellApplication {
      name = keyCfg.signWrapperName;
      runtimeInputs = [pkgs.tpm2-tools pkgs.coreutils];
      text = ''
        set -euo pipefail
        if [ $# -ne 1 ]; then
          echo "usage: ${keyCfg.signWrapperName} <file>" >&2
          exit 2
        fi

        tmpsig="$(mktemp)"
        ${lib.optionalString (keyCfg.pcrPolicy != []) ''
          session="$(mktemp)"
          trap 'tpm2_flushcontext "$session" >/dev/null 2>&1 || true; rm -f "$tmpsig" "$session"' EXIT
          tpm2_startauthsession -S "$session" --policy-session
          tpm2_policypcr -S "$session" -l ${pcrSpec} >/dev/null
          # tpm2_sign's `-o -` silently produces empty output on this
          # tpm2-tools version; use a tempfile.
          tpm2_sign -c ${keyCfg.handle} -p "session:$session" \
            ${spec.tpmSignHashArg} -o "$tmpsig" "$1"
        ''}
        ${lib.optionalString (keyCfg.pcrPolicy == []) ''
          trap 'rm -f "$tmpsig"' EXIT
          tpm2_sign -c ${keyCfg.handle} ${spec.tpmSignHashArg} -o "$tmpsig" "$1"
        ''}
        ${extractRawSigScript}  "$tmpsig"
      '';
    };

  # Per-key provisioning systemd unit. Idempotent: if the handle already
  # holds a key, skip createprimary and just re-export the pubkey.
  mkProvisionService = name: keyCfg: let
    spec = algoSpec keyCfg.algorithm;
    dir = pubkeyDirOf name;
    pemPath = "${dir}/pubkey.pem";
    rawPath = "${dir}/pubkey.raw";
    pcrList = lib.concatStringsSep "," (map (i: "${i}") keyCfg.pcrPolicy);
    pcrSpec = "sha256:${pcrList}";
    # createprimary attribute string. With PCR policy, drop `userwithauth`
    # so the TPM enforces policy-only auth.
    primaryAttrs =
      if keyCfg.pcrPolicy == []
      then "fixedtpm|fixedparent|sensitivedataorigin|userwithauth|sign"
      else "fixedtpm|fixedparent|sensitivedataorigin|sign";
    policyClause = lib.optionalString (keyCfg.pcrPolicy != []) "--policy /tmp/nixfleet-tpm-keyslot-${name}.policy";
  in {
    description = "Provision TPM-backed ${keyCfg.algorithm} keyslot '${name}' at ${keyCfg.handle}";
    wantedBy = ["multi-user.target"];
    after = ["tpm2-abrmd.service" "basic.target"];
    wants = ["tpm2-abrmd.service"];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    path = [pkgs.tpm2-tools pkgs.openssl pkgs.coreutils];
    script = ''
      set -euo pipefail
      mkdir -p ${dir}
      chmod 0755 ${dir}

      extract_raw() {
        ${spec.extractRawCmd pemPath rawPath}
        chmod 644 ${pemPath} ${rawPath}
      }

      if tpm2_readpublic -c ${keyCfg.handle} -f pem -o ${pemPath} 2>/dev/null; then
        extract_raw
        echo "Keyslot '${name}' already persisted at ${keyCfg.handle}"
        exit 0
      fi

      ${lib.optionalString (keyCfg.pcrPolicy != []) ''
        # Compute PCR policy digest before createprimary so the new key is
        # bound to it. tpm2_createpolicy uses an internal trial session.
        tpm2_createpolicy --policy-pcr \
          -l ${pcrSpec} \
          -L /tmp/nixfleet-tpm-keyslot-${name}.policy
      ''}

      tpm2_createprimary \
        --hierarchy o \
        ${spec.createPrimaryArgs} \
        --attributes '${primaryAttrs}' \
        ${policyClause} \
        --key-context /tmp/nixfleet-tpm-keyslot-${name}.ctx
      tpm2_evictcontrol --hierarchy o \
        --object-context /tmp/nixfleet-tpm-keyslot-${name}.ctx \
        ${keyCfg.handle}
      tpm2_readpublic -c ${keyCfg.handle} -f pem -o ${pemPath}
      extract_raw
      rm -f /tmp/nixfleet-tpm-keyslot-${name}.ctx /tmp/nixfleet-tpm-keyslot-${name}.policy
      echo "${keyCfg.algorithm} keyslot '${name}' provisioned at ${keyCfg.handle}${
        lib.optionalString (keyCfg.pcrPolicy != []) " (PCR policy: ${pcrSpec})"
      }"
    '';
  };

  # All sign wrapper packages, indexed by key name.
  signWrappers = lib.mapAttrs mkSignWrapper effectiveKeys;
in {
  imports = [./options.nix];

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = let
          handles = lib.mapAttrsToList (_: k: k.handle) effectiveKeys;
          uniq = lib.unique handles;
        in
          lib.length handles == lib.length uniq;
        message = ''
          nixfleet.keyslots.tpm: each named keyslot must have a unique
          persistent handle. Got: ${
            lib.concatStringsSep ", " (lib.mapAttrsToList (n: k: "${n}=${k.handle}") effectiveKeys)
          }
        '';
      }
    ];

    security.tpm2 = {
      enable = true;
      tctiEnvironment.enable = true;
    };

    environment.systemPackages =
      [pkgs.tpm2-tools]
      ++ (lib.attrValues signWrappers);

    # Per-key readOnly outputs at the parent level (avoids submodule
    # write-back recursion). Indexed by key name; legacy entry included
    # when cfg.handle is set.
    nixfleet.keyslots.tpm.signWrappers = signWrappers;
    nixfleet.keyslots.tpm.pubkeyDirs = lib.mapAttrs (name: _: pubkeyDirOf name) effectiveKeys;

    # Legacy singleton output for back-compat consumers (e.g. fleet's
    # CI runner). Always available when the scope is enabled.
    nixfleet.keyslots.tpm.signWrapperPackage = signWrappers.legacy;

    systemd.services =
      lib.mapAttrs' (
        name: keyCfg: lib.nameValuePair "nixfleet-tpm-keyslot-provision-${name}" (mkProvisionService name keyCfg)
      )
      effectiveKeys;

    nixfleet.persistence.directories = [
      {
        directory = cfg.exportPubkeyDir;
        mode = "0755";
      }
    ];
  };
}

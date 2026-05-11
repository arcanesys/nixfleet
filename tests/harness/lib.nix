{
  lib,
  pkgs,
  inputs,
}: let
  # LOADBEARING: explicit X.509 extensions; CP's WebPkiClientVerifier strictly checks CA basicConstraints/keyCertSign, client/server EKU.
  mkTlsCerts = {hostnames ? ["cp" "agent-01" "agent-02"]}:
    pkgs.runCommand "nixfleet-harness-test-certs" {
      nativeBuildInputs = [pkgs.openssl];
    } ''
      mkdir -p $out

      cat > $out/ca.cnf <<'EOF'
      [req]
      distinguished_name = dn
      prompt = no
      x509_extensions = ca_ext

      [dn]
      CN = nixfleet-test-ca

      [ca_ext]
      basicConstraints = critical, CA:TRUE
      keyUsage = critical, keyCertSign, cRLSign, digitalSignature
      subjectKeyIdentifier = hash
      EOF
      cat > $out/server-ext.cnf <<'EOF'
      basicConstraints = critical, CA:FALSE
      keyUsage = critical, digitalSignature, keyEncipherment
      extendedKeyUsage = serverAuth
      subjectAltName = DNS:cp, DNS:localhost
      authorityKeyIdentifier = keyid
      EOF
      cat > $out/client-ext.cnf <<'EOF'
      basicConstraints = critical, CA:FALSE
      keyUsage = critical, digitalSignature, keyEncipherment
      extendedKeyUsage = clientAuth
      authorityKeyIdentifier = keyid
      EOF

      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
        -keyout $out/ca-key.pem -out $out/ca.pem -days 365 -nodes \
        -config $out/ca.cnf

      openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
        -keyout $out/cp-key.pem -out $out/cp-csr.pem -nodes \
        -subj '/CN=cp'
      openssl x509 -req -in $out/cp-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
        -CAcreateserial -out $out/cp-cert.pem -days 365 \
        -extfile $out/server-ext.cnf

      # Filter "cp" so iterating doesn't overwrite the SAN-bearing server
      # cert with a SAN-less client cert (rustls requires SANs).
      ${lib.concatMapStringsSep "\n" (h: ''
          openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
            -keyout $out/${h}-key.pem -out $out/${h}-csr.pem -nodes \
            -subj "/CN=${h}"
          openssl x509 -req -in $out/${h}-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
            -CAcreateserial -out $out/${h}-cert.pem -days 365 \
            -extfile $out/client-ext.cnf
        '')
        (lib.filter (h: h != "cp") hostnames)}

      rm -f $out/*-csr.pem $out/*.srl $out/*-ext.cnf $out/ca.cnf
    '';

  mkHarnessCerts = {hostnames ? ["cp" "agent-01" "agent-02"]}:
    mkTlsCerts {inherit hostnames;};

  # LOADBEARING: hypervisor=qemu (not cloud-hypervisor); harness needs user-net bridge-less guest-to-host NAT, CH only does tap.
  microvmGuestDefaults = {
    hypervisor = "qemu";
    mem = 256;
    vcpu = 1;
    shares = [
      {
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
        tag = "ro-store";
        proto = "virtiofs";
      }
    ];
    # LOADBEARING: user-mode networking — every guest sees the host via 10.0.2.2; guest-to-guest needs tap/bridge.
    interfaces = [
      {
        type = "user";
        id = "vm-net";
        mac = "02:00:00:00:00:01";
      }
    ];
  };

  # LOADBEARING: CP runs on host VM (not microVM); user-net microVMs can't reach each other, but share host gateway.
  mkCpHostModule = {
    testCerts,
    resolvedJsonPath,
  }: {
    imports = [./nodes/cp.nix];
    _module.args = {inherit testCerts resolvedJsonPath;};
  };

  mkSignedCpHostModule = {
    testCerts,
    signedFixture,
  }: {
    imports = [./nodes/cp-signed.nix];
    _module.args = {inherit testCerts signedFixture;};
  };

  mkRealCpHostModule = {
    testCerts,
    signedFixture,
    cpPkg,
    revocationsFixture ? null,
  }: {
    imports = [./nodes/cp-real.nix];
    _module.args = {inherit testCerts signedFixture cpPkg revocationsFixture;};
  };

  mkAgentNode = {
    testCerts,
    hostName,
    controlPlaneHost ? "10.0.2.2",
    controlPlanePort ? 8443,
    extraModules ? [],
  }: {
    imports =
      [
        ./nodes/agent.nix
      ]
      ++ extraModules;

    _module.args = {
      inherit testCerts controlPlaneHost controlPlanePort;
      harnessMicrovmDefaults = microvmGuestDefaults;
      agentHostName = hostName;
    };

    networking.hostName = hostName;
    system.stateVersion = lib.mkDefault "24.11";
  };

  mkVerifyingAgentNode = {
    testCerts,
    hostName,
    signedFixture,
    verifyArtifactPkg,
    controlPlaneHost ? "10.0.2.2",
    controlPlanePort ? 8443,
    now ? signedFixture.now,
    freshnessWindowSecs ? 604800,
    extraModules ? [],
  }: {
    imports =
      [
        ./nodes/agent-verify.nix
      ]
      ++ extraModules;

    _module.args = {
      inherit
        testCerts
        controlPlaneHost
        controlPlanePort
        signedFixture
        verifyArtifactPkg
        now
        freshnessWindowSecs
        ;
      harnessMicrovmDefaults = microvmGuestDefaults;
      agentHostName = hostName;
    };

    networking.hostName = hostName;
    system.stateVersion = lib.mkDefault "24.11";
  };

  mkRealAgentNode = {
    testCerts,
    signedFixture,
    agentPkg,
    hostName,
    controlPlaneHost ? "10.0.2.2",
    controlPlanePort ? 8443,
    pollIntervalSecs ? 10,
    # OpenSSH-format private key (e.g. ${agentKeypairs.agent-01}/private.openssh).
    # When set, lands at /etc/ssh/ssh_host_ed25519_key on the VM so the
    # agent's evidence_signer signs last_confirmed_at attestations with
    # a key matching the host's declared pubkey in fleet.nix (#43
    # contract).
    sshHostKey ? null,
    extraModules ? [],
  }: {
    imports =
      [
        ./nodes/agent-real.nix
      ]
      ++ extraModules;

    _module.args = {
      inherit inputs testCerts controlPlaneHost controlPlanePort agentPkg signedFixture pollIntervalSecs sshHostKey;
      harnessMicrovmDefaults = microvmGuestDefaults;
      agentHostName = hostName;
    };

    networking.hostName = hostName;
    system.stateVersion = lib.mkDefault "24.11";
  };

  # LOADBEARING: overrides /run/current-system + seeds last_confirmed_at so agent's closure_hash matches fleet hosts[*].closureHash.
  convergencePreseedModule = {
    closureHash,
    attestedAt ? "2026-04-01T00:00:00Z",
  }: {pkgs, ...}: {
    systemd.services.harness-agent-preseed = {
      description = "Pre-seed agent state-dir + override /run/current-system for convergence";
      wantedBy = ["multi-user.target"];
      before = ["nixfleet-agent.service"];
      after = ["local-fs.target"];
      # FOOTGUN: without requiredBy, preseed failure silently false-passes convergence assertions (read_last_confirmed -> Ok(None)).
      requiredBy = ["nixfleet-agent.service"];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        set -euo pipefail

        # Symlink target need not exist; the agent reports its basename.
        ${pkgs.coreutils}/bin/ln -sfn \
          /tmp/${closureHash} /run/current-system

        ${pkgs.coreutils}/bin/mkdir -p /var/lib/nixfleet-agent
        ${pkgs.coreutils}/bin/chmod 0700 /var/lib/nixfleet-agent
        ${pkgs.coreutils}/bin/printf '%s\n%s\n' \
          '${closureHash}' '${attestedAt}' \
          > /var/lib/nixfleet-agent/last_confirmed_at
        ${pkgs.coreutils}/bin/chmod 0600 \
          /var/lib/nixfleet-agent/last_confirmed_at
      '';
    };
  };

  testScriptPrelude = ''
    import time

    def wait_for_journal_match(
        host,
        *,
        since_cursor,
        unit,
        pattern,
        timeout=60,
        sleep_secs=2,
        label=None,
    ):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            rc, _ = host.execute(
                "journalctl -u " + unit
                + " --since='" + since_cursor + "' --no-pager "
                + "| grep -E " + repr(pattern)
            )
            if rc == 0:
                return
            time.sleep(sleep_secs)
        dump = host.succeed(
            "journalctl -u " + unit
            + " --since='" + since_cursor + "' --no-pager"
        )
        msg = label if label is not None else pattern
        raise Exception(
            msg + " did not appear in " + unit + " within "
            + str(timeout) + "s\n=== " + unit + " journal ===\n"
            + dump + "\n=== end ==="
        )
  '';

  # LOADBEARING: mirrors microvmGuestDefaults.mem so host-RAM sizing is correct (CP + N guests + headroom).
  guestMemMB = 256;

  mkFleetScenario = {
    name,
    cpHostModule,
    agents,
    testScript,
    timeout ? 600,
    hostMemoryMB ? null,
    # GOTCHA: when false, testScript starts each microvm@<name>.service manually for staggered boot at high N.
    agentVmAutostart ? true,
  }: let
    agentCount = builtins.length (builtins.attrNames agents);
    autoHostMemoryMB = lib.max 4096 (1024 + agentCount * guestMemMB + 2048);
    resolvedHostMemoryMB =
      if hostMemoryMB != null
      then hostMemoryMB
      else autoHostMemoryMB;
  in
    pkgs.testers.runNixOSTest {
      inherit name;
      node.specialArgs = {inherit inputs;};

      nodes.host = {pkgs, ...}: {
        imports = [
          inputs.microvm.nixosModules.host
          cpHostModule
        ];

        virtualisation = {
          cores = 2;
          memorySize = resolvedHostMemoryMB;
          diskSize = 8192;
          qemu.options = [
            "-cpu"
            "kvm64,+svm,+vmx"
          ];
        };

        microvm.vms =
          lib.mapAttrs (_: mod: {
            config = mod;
            autostart = agentVmAutostart;
          })
          agents;

        environment.systemPackages = [pkgs.jq pkgs.curl];
      };

      testScript = testScriptPrelude + "\n" + testScript;
      meta.timeout = timeout;
    };
in {
  inherit
    convergencePreseedModule
    mkAgentNode
    mkCpHostModule
    mkFleetScenario
    mkHarnessCerts
    mkRealAgentNode
    mkRealCpHostModule
    mkSignedCpHostModule
    mkVerifyingAgentNode
    microvmGuestDefaults
    ;
}

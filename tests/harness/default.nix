{
  lib,
  pkgs,
  inputs,
  nixfleet-canonicalize ? null,
  nixfleet-verify-artifact ? null,
  nixfleet-control-plane ? null,
  nixfleet-agent ? null,
  nixfleet-cli ? null,
}: let
  harnessLib = import ./lib.nix {inherit lib pkgs inputs;};

  certHostnames = [
    "cp"
    "agent-01"
    "agent-02"
    "agent-03"
    "agent-04"
    "agent-05"
    "agent-06"
    "agent-07"
    "agent-08"
    "agent-09"
    "agent-10"
  ];

  sharedCerts = harnessLib.mkHarnessCerts {
    hostnames = certHostnames;
  };

  mkAgentNames = n:
    map (i: "agent-${lib.fixedWidthString 2 "0" (toString i)}") (lib.range 1 n);

  scenarioArgs = {
    inherit lib pkgs inputs harnessLib;
    testCerts = sharedCerts;
    resolvedJsonPath = ./fixtures/fleet-resolved.json;
    # Forward declaration - `agentKeypairs` is bound a few lines below;
    # the `let ... in` lets us reference it here without rearranging
    # the whole block.
    inherit agentKeypairs;
  };

  # Deterministic per-agent keypair. Each agent the harness exercises
  # (or that needs an attested last_confirmed_at / enrolment CSR
  # validated against fleet.nix) consumes its keypair from here.
  mkAgentKeypair = hostName:
    import ./fixtures/agent-keypair {inherit pkgs hostName;};

  # Canonical attrset of agent keypairs the harness uses. agent-01 and
  # agent-02 power most scenarios; agent-99 is enroll-replay's
  # never-deployed CN.
  agentKeypairs = {
    agent-01 = mkAgentKeypair "agent-01";
    agent-02 = mkAgentKeypair "agent-02";
    agent-99 = mkAgentKeypair "agent-99";
  };

  # OpenSSH-format pubkey strings from each keypair fixture, ready to
  # paste into hosts.<name>.pubkey via the `agentPubkeys` argument on
  # the signedFixture builders.
  agentPubkeys =
    lib.mapAttrs
    (_: kp: builtins.readFile "${kp}/public.openssh")
    agentKeypairs;

  signedFixture =
    if nixfleet-canonicalize == null
    then
      throw ''
        tests/harness: signedFixture requires `nixfleet-canonicalize` to be
        passed in. Wire it via `modules/tests/harness.nix` or call sites
        that have the flake's `packages.<system>.nixfleet-canonicalize`.
      ''
    else
      import ./fixtures/signed {
        inherit lib pkgs nixfleet-canonicalize;
        # enroll-replay only - the other scenarios use convergedSignedFixture.
        agentPubkeys = {inherit (agentPubkeys) agent-99;};
      };

  agenixFixture = import ./fixtures/agenix {inherit pkgs;};

  # Combined trust.json carries both orgRootKey (enrol) and ciReleaseKey
  # (signed fleet bytes) so CP boots against harness fixtures with one file.
  orgRootKeyFixture =
    if nixfleet-canonicalize == null
    then null
    else let
      bareKey = import ./fixtures/org-root-key {
        inherit pkgs;
      };
    in
      pkgs.runCommand "nixfleet-harness-org-root-key-with-trust" {} ''
        set -euo pipefail
        mkdir -p "$out"
        cp ${bareKey}/private.pem "$out/private.pem"
        cp ${bareKey}/pubkey.b64 "$out/pubkey.b64"
        org_pub=$(cat ${bareKey}/pubkey.b64)
        ci_pub=$(cat ${signedFixture}/verify-pubkey.b64)
        cat > "$out/trust.json" <<EOF
        {
          "schemaVersion": 1,
          "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": "$ci_pub" },
            "previous": null,
            "rejectBefore": null
          },
          "cacheKeys": [],
          "orgRootKey": {
            "current": { "algorithm": "ed25519", "public": "$org_pub" },
            "previous": null,
            "rejectBefore": null
          }
        }
        EOF
      '';

  probeFixture =
    if nixfleet-canonicalize == null
    then null
    else import ./fixtures/probe {inherit pkgs nixfleet-canonicalize;};

  rolloutManifestFixture =
    if nixfleet-canonicalize == null
    then null
    else import ./fixtures/rollout-manifest {inherit pkgs nixfleet-canonicalize;};

  # Shares seedSalt with signedFixture so it verifies under the same trust.json.
  revocationsFixture =
    if nixfleet-canonicalize == null
    then null
    else import ./fixtures/signed/revocations.nix {inherit pkgs nixfleet-canonicalize;};

  # Without an injected closureHash, dispatch silently yields NoDeclaration;
  # this variant lets convergence-gated assertions actually progress.
  convergedClosureHash = "0000000000000000000000000000000000000000-harness-stub";
  convergedSignedFixture =
    if nixfleet-canonicalize == null
    then null
    else
      import ./fixtures/signed {
        inherit lib pkgs nixfleet-canonicalize;
        derivationName = "nixfleet-harness-converged-signed-fixture";
        hostClosureHashes = {
          "agent-01" = convergedClosureHash;
          "agent-02" = convergedClosureHash;
        };
        # post-#43: attested last_confirmed_at verifies against
        # host.pubkey. Without these the soak-state recovery path bails
        # silently and the post-CP-wipe convergence assertions hang.
        agentPubkeys = {inherit (agentPubkeys) agent-01 agent-02;};
      };

  # Signed far enough in the past that the agent's per-channel freshness
  # check fires while CP (with a huge --freshness-window-secs) still dispatches.
  staleFixture =
    if nixfleet-canonicalize == null
    then null
    else
      import ./fixtures/signed {
        inherit lib pkgs nixfleet-canonicalize;
        signedAt = "2025-01-01T00:00:00Z";
        # Smallest mk-fleet-permissible window (2 x signingInterval=60).
        freshnessWindowMinutes = 120;
        seedSalt = "nixfleet-harness-stale-fixture-2025";
        derivationName = "nixfleet-harness-stale-fixture";
      };

  signedRoundtripScenario =
    if nixfleet-verify-artifact == null
    then
      throw ''
        tests/harness: fleet-harness-signed-roundtrip requires
        `nixfleet-verify-artifact` to be passed in. Wire it via
        `modules/tests/harness.nix` using the crane-built package.
      ''
    else
      import ./scenarios/signed-roundtrip.nix (scenarioArgs
        // {
          inherit signedFixture;
          verifyArtifactPkg = nixfleet-verify-artifact;
        });

  teardownScenario =
    if nixfleet-control-plane == null || nixfleet-agent == null
    then
      throw ''
        tests/harness: fleet-harness-teardown requires both
        `nixfleet-control-plane` and `nixfleet-agent` to be passed
        in. Wire via `modules/tests/harness.nix` using the crane-
        built packages.
      ''
    else
      import ./scenarios/teardown.nix (scenarioArgs
        // {
          signedFixture = convergedSignedFixture;
          inherit revocationsFixture;
          closureHash = convergedClosureHash;
          cpPkg = nixfleet-control-plane;
          agentPkg = nixfleet-agent;
        });

  secretHygieneScenario =
    if nixfleet-control-plane == null || nixfleet-agent == null
    then
      throw ''
        tests/harness: fleet-harness-secret-hygiene requires both
        `nixfleet-control-plane` and `nixfleet-agent`. Wire via
        modules/tests/harness.nix.
      ''
    else
      import ./scenarios/secret-hygiene.nix (scenarioArgs
        // {
          signedFixture = convergedSignedFixture;
          closureHash = convergedClosureHash;
          inherit agenixFixture;
          cpPkg = nixfleet-control-plane;
          agentPkg = nixfleet-agent;
        });

  staleTargetScenario =
    if nixfleet-control-plane == null || nixfleet-agent == null || staleFixture == null
    then
      throw ''
        tests/harness: fleet-harness-stale-target requires
        `nixfleet-control-plane`, `nixfleet-agent`, and
        `nixfleet-canonicalize` (for staleFixture) to be passed in.
      ''
    else
      import ./scenarios/stale-target.nix (scenarioArgs
        // {
          staleFixture = staleFixture;
          cpPkg = nixfleet-control-plane;
          agentPkg = nixfleet-agent;
        });

  bootRecoveryScenario =
    if nixfleet-control-plane == null || nixfleet-agent == null
    then
      throw ''
        tests/harness: fleet-harness-boot-recovery requires both
        `nixfleet-control-plane` and `nixfleet-agent` to be passed in.
      ''
    else
      import ./scenarios/boot-recovery.nix (scenarioArgs
        // {
          signedFixture = convergedSignedFixture;
          closureHash = convergedClosureHash;
          cpPkg = nixfleet-control-plane;
          agentPkg = nixfleet-agent;
        });

  auditorChainScenario =
    if nixfleet-verify-artifact == null || probeFixture == null
    then
      throw ''
        tests/harness: fleet-harness-auditor-chain requires both
        `nixfleet-canonicalize` (for probeFixture) and
        `nixfleet-verify-artifact`. Wire via modules/tests/harness.nix.
      ''
    else
      import ./scenarios/auditor-chain.nix {
        inherit pkgs probeFixture;
        verifyArtifactPkg = nixfleet-verify-artifact;
      };

  corruptionRejectionScenario =
    if nixfleet-verify-artifact == null
    then
      throw ''
        tests/harness: fleet-harness-corruption-rejection requires
        `nixfleet-verify-artifact`. Wire via modules/tests/harness.nix.
      ''
    else
      import ./scenarios/corruption-rejection.nix {
        inherit pkgs signedFixture;
        verifyArtifactPkg = nixfleet-verify-artifact;
      };

  futureDatedRejectionScenario =
    if nixfleet-verify-artifact == null
    then
      throw ''
        tests/harness: fleet-harness-future-dated-rejection requires
        `nixfleet-verify-artifact`. Wire via modules/tests/harness.nix.
      ''
    else
      import ./scenarios/future-dated-rejection.nix {
        inherit pkgs signedFixture;
        verifyArtifactPkg = nixfleet-verify-artifact;
      };

  enrollReplayScenario =
    if nixfleet-control-plane == null || nixfleet-cli == null || orgRootKeyFixture == null
    then
      throw ''
        tests/harness: fleet-harness-enroll-replay requires
        `nixfleet-control-plane`, `nixfleet-cli`, and
        `nixfleet-canonicalize` (for the org-root key fixture).
        Wire via modules/tests/harness.nix.
      ''
    else
      import ./scenarios/enroll-replay.nix (scenarioArgs
        // {
          inherit signedFixture orgRootKeyFixture;
          cpPkg = nixfleet-control-plane;
          cliPkg = nixfleet-cli;
        });

  concurrentCheckinScenario =
    if nixfleet-control-plane == null
    then
      throw ''
        tests/harness: fleet-harness-concurrent-checkin requires
        `nixfleet-control-plane` to be passed in.
      ''
    else
      import ./scenarios/concurrent-checkin.nix (scenarioArgs
        // {
          inherit signedFixture;
          cpPkg = nixfleet-control-plane;
        });

  manifestTamperRejectionScenario =
    if nixfleet-verify-artifact == null || rolloutManifestFixture == null
    then
      throw ''
        tests/harness: fleet-harness-manifest-tamper-rejection requires
        both `nixfleet-canonicalize` (for the rolloutManifestFixture)
        and `nixfleet-verify-artifact`. Wire via modules/tests/harness.nix.
      ''
    else
      import ./scenarios/manifest-tamper-rejection.nix {
        inherit pkgs rolloutManifestFixture;
        verifyArtifactPkg = nixfleet-verify-artifact;
      };

  moduleRolloutsWireScenario =
    if nixfleet-control-plane == null || rolloutManifestFixture == null
    then
      throw ''
        tests/harness: fleet-harness-module-rollouts-wire requires both
        `nixfleet-control-plane` and `nixfleet-canonicalize` (for the
        rolloutManifestFixture) to be passed in. Wire via
        modules/tests/harness.nix.
      ''
    else
      import ./scenarios/module-rollouts-wire.nix {
        inherit lib pkgs inputs rolloutManifestFixture signedFixture;
        testCerts = sharedCerts;
        cpPkg = nixfleet-control-plane;
      };

  deadlineExpiryScenario =
    if nixfleet-control-plane == null
    then
      throw ''
        tests/harness: fleet-harness-deadline-expiry requires
        `nixfleet-control-plane` to be passed in.
      ''
    else
      import ./scenarios/deadline-expiry.nix (scenarioArgs
        // {
          inherit signedFixture;
          cpPkg = nixfleet-control-plane;
        });

  rollbackPolicyScenario =
    if nixfleet-control-plane == null || nixfleet-agent == null
    then
      throw ''
        tests/harness: fleet-harness-rollback-policy requires both
        `nixfleet-control-plane` and `nixfleet-agent`. Wire via
        modules/tests/harness.nix.
      ''
    else let
      # Only onHealthFailure flips vs convergedSignedFixture; that unlocks
      # compute_rollback_signal.
      rollbackHaltSignedFixture =
        if nixfleet-canonicalize == null
        then null
        else
          import ./fixtures/signed {
            inherit lib pkgs nixfleet-canonicalize;
            hostClosureHashes = {
              "agent-01" = convergedClosureHash;
              "agent-02" = convergedClosureHash;
            };
            onHealthFailure = "rollback-and-halt";
            derivationName = "nixfleet-harness-signed-fixture-rollback-halt";
          };
    in
      if rollbackHaltSignedFixture == null
      then
        throw ''
          tests/harness: fleet-harness-rollback-policy requires
          `nixfleet-canonicalize` for the rollback-halt fixture.
        ''
      else
        import ./scenarios/rollback-policy.nix (scenarioArgs
          // {
            signedFixture = rollbackHaltSignedFixture;
            closureHash = convergedClosureHash;
            cpPkg = nixfleet-control-plane;
            agentPkg = nixfleet-agent;
          });

  mkFleetNScenario = n:
    import ./scenarios/smoke.nix (scenarioArgs
      // {
        agentNames = mkAgentNames n;
        scenarioName = "fleet-harness-fleet-${toString n}";
      });
in {
  fleet-harness-smoke = import ./scenarios/smoke.nix scenarioArgs;

  fleet-harness-signed-roundtrip = signedRoundtripScenario;

  fleet-harness-teardown = teardownScenario;

  fleet-harness-stale-target = staleTargetScenario;

  fleet-harness-boot-recovery = bootRecoveryScenario;

  fleet-harness-deadline-expiry = deadlineExpiryScenario;

  fleet-harness-auditor-chain = auditorChainScenario;

  fleet-harness-corruption-rejection = corruptionRejectionScenario;

  fleet-harness-future-dated-rejection = futureDatedRejectionScenario;

  fleet-harness-enroll-replay = enrollReplayScenario;

  fleet-harness-concurrent-checkin = concurrentCheckinScenario;

  fleet-harness-manifest-tamper-rejection = manifestTamperRejectionScenario;

  fleet-harness-module-rollouts-wire = moduleRolloutsWireScenario;

  fleet-harness-secret-hygiene = secretHygieneScenario;

  fleet-harness-rollback-policy = rollbackPolicyScenario;

  fleet-harness-fleet-2 = mkFleetNScenario 2;
  fleet-harness-fleet-5 = mkFleetNScenario 5;
  fleet-harness-fleet-10 = mkFleetNScenario 10;

  inherit signedFixture;
  inherit agenixFixture;
  inherit probeFixture;
  inherit revocationsFixture;
  inherit rolloutManifestFixture;
}

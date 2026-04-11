# Phase 3 VM scenario tests. Each subtest is an independently buildable
# `testers.nixosTest` so a failure in one does not mask another.
#
# Run any subtest with:
#   nix build .#checks.x86_64-linux.vm-fleet-<name> --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};
    mkTlsCerts = import ./_lib/tls-certs.nix {inherit pkgs lib;};

    mkTestNode = helpers.mkTestNode {
      inherit inputs;
      hostSpecModule = ../_shared/host-spec-module.nix;
    };

    defaultTestSpec = helpers.defaultTestSpec;

    # High-level scenario helpers pre-bound with mkTestNode + defaultTestSpec
    # so scenario files only pass scenario-specific args (testCerts,
    # hostName, tags, healthChecks, …).
    mkCpNode = args:
      helpers.mkCpNode ({inherit mkTestNode defaultTestSpec;} // args);
    mkAgentNode = args:
      helpers.mkAgentNode ({inherit mkTestNode defaultTestSpec;} // args);
    testPrelude = helpers.testPrelude;

    scenarioArgs = {
      inherit pkgs lib mkTestNode defaultTestSpec mkTlsCerts;
      inherit mkCpNode mkAgentNode testPrelude;
    };

    subtests = {
      vm-fleet-tag-sync = import ./_vm-fleet-scenarios/tag-sync.nix scenarioArgs;
      vm-fleet-release = import ./_vm-fleet-scenarios/release.nix scenarioArgs;
      vm-fleet-bootstrap = import ./_vm-fleet-scenarios/bootstrap.nix scenarioArgs;
      vm-fleet-deploy-ssh = import ./_vm-fleet-scenarios/deploy-ssh.nix scenarioArgs;
      vm-fleet-apply-failure = import ./_vm-fleet-scenarios/apply-failure.nix scenarioArgs;
      vm-fleet-revert = import ./_vm-fleet-scenarios/revert.nix scenarioArgs;
      vm-fleet-timeout = import ./_vm-fleet-scenarios/timeout.nix scenarioArgs;
      vm-fleet-poll-retry = import ./_vm-fleet-scenarios/poll-retry.nix scenarioArgs;
      vm-fleet-mtls-missing = import ./_vm-fleet-scenarios/mtls-missing.nix scenarioArgs;
      vm-fleet-rollback-ssh = import ./_vm-fleet-scenarios/rollback-ssh.nix scenarioArgs;
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = subtests;
    };
}

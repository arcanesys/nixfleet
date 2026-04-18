# Per-scenario VM tests. Each subtest is an independently buildable
# `testers.runNixOSTest` so a failure in one does not mask another.
#
# Run any subtest with:
#   nix build .#checks.x86_64-linux.vm-fleet-<name> --no-link
{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    system,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs inputs;};
    inherit (helpers) mkTlsCerts sharedTestCerts;

    mkTestNode = helpers.mkTestNode {
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
    tlsCertsModule = helpers.tlsCertsModule;

    # Every scenario receives `testCerts = sharedTestCerts` by default.
    # Because the store path is identical, mkCpNode / mkAgentNode produce
    # IDENTICAL system closures across scenarios that differ only in
    # testScript. Nix dedupes those closures, so each unique node shape
    # is built at most once across the entire VM suite rather than once
    # per scenario. `mkTlsCerts` is still exposed for any future scenario
    # that genuinely needs a different CA / hostname shape.
    scenarioArgs = {
      inherit pkgs lib inputs mkTestNode defaultTestSpec mkTlsCerts;
      inherit mkCpNode mkAgentNode testPrelude tlsCertsModule;
      testCerts = sharedTestCerts;
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
      vm-fleet-mtls-cn-mismatch = import ./_vm-fleet-scenarios/mtls-cn-mismatch.nix scenarioArgs;
      vm-fleet-rollback-ssh = import ./_vm-fleet-scenarios/rollback-ssh.nix scenarioArgs;
      vm-fleet-agent-rebuild = import ./_vm-fleet-scenarios/agent-rebuild.nix scenarioArgs;
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = subtests;
    };
}

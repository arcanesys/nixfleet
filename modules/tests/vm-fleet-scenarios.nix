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

    subtests = {
      vm-fleet-tag-sync = import ./_vm-fleet-scenarios/tag-sync.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-release = import ./_vm-fleet-scenarios/release.nix {
        inherit pkgs lib mkTestNode defaultTestSpec mkTlsCerts;
      };
      # Task 20: vm-fleet-bootstrap = ...
      # Task 21: vm-fleet-deploy-ssh = ...
      # Task 22a: vm-fleet-apply-failure = ...
      # Task 22b: vm-fleet-revert = ...
      # Task 22c: vm-fleet-timeout = ...
      # Task 23: vm-fleet-poll-retry = ...
      # Task 24: vm-fleet-mtls-missing = ...
      # Task 25: vm-fleet-rollback-ssh = ...
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = subtests;
    };
}

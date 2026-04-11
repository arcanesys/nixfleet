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
      vm-fleet-bootstrap = import ./_vm-fleet-scenarios/bootstrap.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-deploy-ssh = import ./_vm-fleet-scenarios/deploy-ssh.nix {
        inherit pkgs lib mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-apply-failure = import ./_vm-fleet-scenarios/apply-failure.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-revert = import ./_vm-fleet-scenarios/revert.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-timeout = import ./_vm-fleet-scenarios/timeout.nix {
        inherit pkgs lib mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-poll-retry = import ./_vm-fleet-scenarios/poll-retry.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-mtls-missing = import ./_vm-fleet-scenarios/mtls-missing.nix {
        inherit pkgs mkTestNode defaultTestSpec mkTlsCerts;
      };
      vm-fleet-rollback-ssh = import ./_vm-fleet-scenarios/rollback-ssh.nix {
        inherit pkgs lib mkTestNode defaultTestSpec mkTlsCerts;
      };
    };
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = subtests;
    };
}

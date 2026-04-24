{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    ...
  }: let
    craneLib = inputs.crane.mkLib pkgs;
    workspace = import ../crane-workspace.nix {inherit lib craneLib;};
  in {
    inherit (workspace) packages checks;

    apps.agent = {
      type = "app";
      program = "${workspace.packages.nixfleet-agent}/bin/nixfleet-agent";
      meta.description = "NixFleet fleet management agent";
    };

    apps.control-plane = {
      type = "app";
      program = "${workspace.packages.nixfleet-control-plane}/bin/nixfleet-control-plane";
      meta.description = "NixFleet control plane server";
    };

    apps.nixfleet = {
      type = "app";
      program = "${workspace.packages.nixfleet-cli}/bin/nixfleet";
      meta.description = "NixFleet fleet management CLI";
    };

    apps.nixfleet-canonicalize = {
      type = "app";
      program = "${workspace.packages.nixfleet-canonicalize}/bin/nixfleet-canonicalize";
      meta.description = "JCS canonicalizer — invoked by CI before signing (CONTRACTS.md §III)";
    };

    apps.nixfleet-verify-artifact = {
      type = "app";
      program = "${workspace.packages.nixfleet-verify-artifact}/bin/nixfleet-verify-artifact";
      meta.description = "Phase 2 harness CLI — verify a signed fleet.resolved against a trust.json";
    };

    devShells.default = craneLib.devShell {
      checks = workspace.checks;
      packages = with pkgs; [
        cargo-nextest
        rust-analyzer
        git
        age
        bashInteractive
      ];
      shellHook = ''
        export EDITOR=vim
        git config core.hooksPath .githooks 2>/dev/null || true
      '';
    };
  };
}

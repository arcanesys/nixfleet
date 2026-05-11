{inputs, ...}: {
  perSystem = {
    pkgs,
    lib,
    config,
    ...
  }: let
    craneLib = inputs.crane.mkLib pkgs;
    workspace = import ../crane-workspace.nix {inherit lib craneLib;};
  in {
    inherit (workspace) checks;
    packages =
      workspace.packages
      // {
        docs-site =
          pkgs.runCommand "nixfleet-docs-site" {
            nativeBuildInputs = [pkgs.mdbook];
          } ''
            cp -r ${inputs.self} src
            chmod -R u+w src
            cd src

            cp ${config.packages.options-doc} docs/mdbook/src/options.md

            # RFC sources stay in docs/rfcs/; mdbook's {{#include}}
            # preprocessor (in docs/mdbook/src/rfcs/000{1,2,3}-*.md)
            # pulls them in at build time. Single source of truth.

            mdbook build docs/mdbook

            mkdir -p docs/mdbook/book/api
            if [ -d ${workspace.cargoDocs}/share/doc ]; then
              cp -r ${workspace.cargoDocs}/share/doc/. docs/mdbook/book/api/
            else
              cp -r ${workspace.cargoDocs}/. docs/mdbook/book/api/
            fi

            cp -r docs/mdbook/book $out
          '';
      };

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
      meta.description = "Operator CLI - `nixfleet status`, planned: rollout trace + diff";
    };

    apps.nixfleet-canonicalize = {
      type = "app";
      program = "${workspace.packages.nixfleet-canonicalize}/bin/nixfleet-canonicalize";
      meta.description = "JCS canonicalizer - invoked by CI before signing (CONTRACTS.md §III)";
    };

    apps.nixfleet-verify-artifact = {
      type = "app";
      program = "${workspace.packages.nixfleet-verify-artifact}/bin/nixfleet-verify-artifact";
      meta.description = "Harness CLI - verify a signed fleet.resolved against a trust.json";
    };

    apps.nixfleet-release = {
      type = "app";
      program = "${workspace.packages.nixfleet-release}/bin/nixfleet-release";
      meta.description = "Producer for fleet.resolved.json - build → inject closureHash → canonicalize → sign → release (CONTRACTS §I #1)";
    };

    devShells.default = craneLib.devShell {
      checks = workspace.checks;
      packages = with pkgs; [
        cargo-nextest
        rust-analyzer
        git
        age
        bashInteractive
        tokei
        cloc
      ];
      shellHook = ''
        export EDITOR=vim
        git config core.hooksPath .githooks 2>/dev/null || true
      '';
    };
  };
}

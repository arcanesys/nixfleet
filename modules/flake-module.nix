# NixFleet Framework Export
#
# Auto-imported by import-tree. Exposes the framework API.
#
# Exports:
#   flake.lib.nixfleet.mkHost  - the API
#   flake.nixosModules.nixfleet-core - for users who want modules without mkHost
#   flake.diskoTemplates - reusable disk layout templates
#   flakeModules.apps/tests/iso/formatter - for fleet repos (transitional)
{
  inputs,
  lib,
  ...
}: let
  nixfleetLib = import ./_shared/lib/default.nix {inherit inputs lib;};
in {
  options.nixfleet.lib = lib.mkOption {
    type = lib.types.attrs;
    default = nixfleetLib;
    readOnly = true;
    description = "NixFleet library (mkHost)";
  };

  config.flake = {
    # Primary API - nixfleet.lib.mkHost
    lib = nixfleetLib;

    # For consumers who don't want mkHost (just raw modules)
    nixosModules.nixfleet-core = ./core/_nixos.nix;

    # Re-export nixfleet-scopes so consumers don't need a separate input.
    # Usage: inputs.nixfleet.scopes.roles.endpoint
    scopes = inputs.nixfleet-scopes.scopes;

    # Legacy alias (use scopes.disk-templates instead)
    diskoTemplates = inputs.nixfleet-scopes.scopes.disk-templates;

    # Flake templates - nix flake init -t nixfleet
    templates = {
      standalone = {
        path = ../examples/standalone-host;
        description = "Single NixOS machine managed by NixFleet";
      };
      batch = {
        path = ../examples/batch-hosts;
        description = "Batch of identical hosts from a template";
      };
      fleet = {
        path = ../examples/client-fleet;
        description = "Multi-host fleet with flake-parts";
      };
      default = {
        path = ../examples/standalone-host;
        description = "Single NixOS machine managed by NixFleet (default)";
      };
    };

    # Transitional flakeModules for fleet repos
    flakeModules = {
      apps = ./apps.nix;
      tests = {
        imports = [
          ./tests/eval.nix
        ];
      };
      iso = ./iso.nix;
      formatter = ./formatter.nix;
    };
  };
}

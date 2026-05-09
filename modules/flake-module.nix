{
  inputs,
  lib,
  ...
}: let
  nixfleetLib = import ../lib {inherit inputs lib;};
in {
  options.nixfleet.lib = lib.mkOption {
    type = lib.types.attrs;
    default = nixfleetLib;
    readOnly = true;
    description = "NixFleet library (mkHost / mkFleet / mkVmApps / ...)";
  };

  config.flake = {
    lib = nixfleetLib;

    nixosModules.nixfleet-core = ./core/_nixos.nix;

    scopes = {
      persistence = {
        impermanence = ../impls/persistence/impermanence.nix;
      };
      keyslots = {
        tpm = ../impls/keyslots/tpm;
      };
      # GOTCHA: gitea shares the Forgejo API verbatim; same impl serves both.
      gitops = {
        forgejo = import ../impls/gitops/forgejo.nix;
        gitea = import ../impls/gitops/forgejo.nix;
      };
      secrets = ../impls/secrets;
    };
  };
}

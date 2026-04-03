# Example: Agenix secrets wired to framework's resolvedIdentityPaths.
# Prerequisites:
#   - inputs.agenix declared in your fleet's flake.nix
#   - inputs.secrets pointing to your encrypted secrets repo
#
# Usage: add this module to your host's `modules` list in fleet.nix.
{
  config,
  inputs,
  lib,
  ...
}: {
  imports = [inputs.agenix.nixosModules.default];

  # Framework computes identity paths based on hostSpec flags:
  #   - Server: host key only
  #   - Workstation: host key + user key fallback
  age.identityPaths = config.nixfleet.secrets.resolvedIdentityPaths;

  # Org-wide secrets (encrypted to all host keys)
  age.secrets.root-password = {
    file = "${inputs.secrets}/org/root-password.age";
  };

  # Workstation-only secrets
  age.secrets.wifi = lib.mkIf (!config.hostSpec.isServer) {
    file = "${inputs.secrets}/shared/wifi.age";
  };

  # Wire password files to hostSpec
  hostSpec = {
    hashedPasswordFile = config.age.secrets.root-password.path;
    rootHashedPasswordFile = config.age.secrets.root-password.path;
  };
}

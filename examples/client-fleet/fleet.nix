# Example: Acme Corp fleet using NixFleet framework
#
# Roles replace the old posture flags (isServer, isMinimal, isImpermanent).
# Operators scope provides declarative user inventory.
{
  config,
  inputs,
  ...
}: let
  mkHost = config.flake.lib.mkHost;

  # Organization defaults
  acme = {
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
  };

  # Shared operators config - team members deployed on every host
  operatorsModule = {
    nixfleet.operators = {
      primaryUser = "alice";
      rootSshKeys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... alice@acme"
      ];
      users.alice = {
        isAdmin = true;
        homeManager.enable = false;
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... alice@acme"
        ];
      };
      users.bob = {
        isAdmin = false;
        homeManager.enable = false;
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... bob@acme"
        ];
      };
    };
  };
in {
  flake.nixosConfigurations = {
    # Developer workstation
    dev-01 = mkHost {
      hostName = "dev-01";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [
        inputs.nixfleet.scopes.roles.workstation
        operatorsModule
        # ./hosts/dev-01/hardware.nix
        # ./hosts/dev-01/disk-config.nix
        # ./modules/secrets.nix     # Agenix secrets wiring
        # ./modules/backup.nix      # Restic backup
        # ./modules/monitoring.nix  # Prometheus node exporter
        # ./modules/tls.nix         # mTLS (agent <-> CP)
      ];
    };

    # Production server
    prod-web-01 = mkHost {
      hostName = "prod-web-01";
      platform = "x86_64-linux";
      hostSpec = acme;
      modules = [
        inputs.nixfleet.scopes.roles.server
        operatorsModule
        # ./hosts/prod-web-01/hardware.nix
        # ./hosts/prod-web-01/disk-config.nix
        # ./modules/secrets.nix     # Agenix secrets wiring
        # ./modules/backup.nix      # Restic backup
        # ./modules/monitoring.nix  # Prometheus node exporter
        # ./modules/tls.nix         # mTLS (agent <-> CP)
      ];
    };
  };
}

# Minimal test fleet for the NixFleet framework repo.
# These hosts exist to make eval tests pass — they are NOT a real org fleet.
# No secrets, no agenix, no real hardware.
# Fleet-specific hostSpec options (isDev, isGraphical, useHyprland, theme, etc.)
# are NOT available here — those are declared by consuming fleets.
#
# Hosts compose via roles from `arcanesys/nixfleet-scopes`:
# - server role: base + firewall + secrets + monitoring + user (wheel)
# - workstation role: base + firewall + secrets + HM + backup + user (groups)
# - endpoint role: base + secrets; no user (distro owns user model)
# - microvm-guest role: base only; no firewall/user (host owns)
{
  config,
  inputs,
  ...
}: let
  mkHost = config.flake.lib.mkHost;

  scopes = inputs.nixfleet-scopes.scopes;

  # Shared organization defaults — just a let binding, no framework function.
  # Placeholder key for eval tests only. Fleet repos set real keys.
  orgDefaults = {
    userName = "deploy";
    timeZone = "UTC";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
    sshAuthorizedKeys = [
      "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
    ];
  };
in {
  flake.nixosConfigurations = {
    # web-01: server, impermanent root
    web-01 = mkHost {
      hostName = "web-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
        {nixfleet.impermanence.enable = true;}
      ];
    };

    # web-02: second server, impermanent root
    web-02 = mkHost {
      hostName = "web-02";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
        {nixfleet.impermanence.enable = true;}
      ];
    };

    # dev-01: developer workstation, custom user
    dev-01 = mkHost {
      hostName = "dev-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec =
        orgDefaults
        // {
          userName = "alice";
        };
      modules = [
        scopes.roles.workstation
      ];
    };

    # edge-01: minimal edge device (no role — just mkHost mechanism).
    # Represents a "bare" host: gets core/_nixos (nix settings, openssh,
    # root key) but no scope opinions.
    edge-01 = mkHost {
      hostName = "edge-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
    };

    # srv-01: production server
    srv-01 = mkHost {
      hostName = "srv-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
      ];
    };

    # agent-test: exercises agent with tags and health checks
    agent-test = mkHost {
      hostName = "agent-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        {
          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp.test:8080";
            tags = ["web" "production"];
            metricsPort = 9101;
            metricsOpenFirewall = true;
            healthChecks = {
              systemd = [{units = ["nginx"];}];
              http = [{url = "http://localhost:80/health";}];
            };
          };
        }
      ];
    };

    # secrets-test: exercises secrets scope on a server (host key only)
    secrets-test = mkHost {
      hostName = "secrets-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
      ];
    };

    # infra-test: exercises backup + monitoring scopes on a workstation
    infra-test = mkHost {
      hostName = "infra-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        scopes.monitoring
        {
          nixfleet.backup = {
            enable = true;
            schedule = "*-*-* 03:00:00";
            healthCheck.onSuccess = "https://hc-ping.com/test-uuid";
          };
          nixfleet.monitoring.nodeExporter = {
            enable = true;
            openFirewall = true;
          };
        }
      ];
    };

    # cache-test: exercises harmonia binary cache server + client
    cache-test = mkHost {
      hostName = "cache-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
        {
          services.nixfleet-cache-server = {
            enable = true;
            signingKeyFile = "/run/secrets/cache-signing-key";
            openFirewall = true;
          };
          services.nixfleet-cache = {
            enable = true;
            cacheUrl = "http://localhost:5000";
            publicKey = "cache-test:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
          };
        }
      ];
    };

    # microvm-test: exercises MicroVM host infrastructure (bridge, DHCP, NAT)
    microvm-test = mkHost {
      hostName = "microvm-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
        {
          services.nixfleet-microvm-host = {
            enable = true;
          };
        }
      ];
    };

    # backup-restic-test: exercises backup with restic backend
    backup-restic-test = mkHost {
      hostName = "backup-restic-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        {
          nixfleet.backup = {
            enable = true;
            backend = "restic";
            restic = {
              repository = "/mnt/backup/restic";
              passwordFile = "/run/secrets/restic-password";
            };
          };
        }
      ];
    };
  };

  flake.darwinConfigurations = {
    # darwin-agent-test: exercises agent with launchd on Darwin
    darwin-agent-test = mkHost {
      hostName = "darwin-agent-test";
      platform = "aarch64-darwin";
      hostSpec = orgDefaults;
      modules = [
        {
          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp.test:8080";
            tags = ["workstation" "darwin"];
            healthChecks = {
              launchd = [{labels = ["com.example.myservice"];}];
              http = [{url = "http://localhost:8080/health";}];
            };
          };
        }
      ];
    };
  };
}

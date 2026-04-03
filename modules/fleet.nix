# Minimal test fleet for the NixFleet framework repo.
# These hosts exist to make eval tests pass — they are NOT a real org fleet.
# No secrets, no agenix, no real hardware.
# Fleet-specific hostSpec options (isDev, isGraphical, useHyprland, theme, etc.)
# are NOT available here — those are declared by consuming fleets.
{config, ...}: let
  mkHost = config.flake.lib.mkHost;

  # Shared organization defaults — just a let binding, no framework function
  orgDefaults = {
    userName = "deploy";
    timeZone = "UTC";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
    sshAuthorizedKeys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINixfleetTestKeyDoNotUseInProduction"
    ];
  };
in {
  flake.nixosConfigurations = {
    # web-01: default web server, impermanent root
    web-01 = mkHost {
      hostName = "web-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec =
        orgDefaults
        // {
          isImpermanent = true;
        };
    };

    # web-02: second web server, impermanent root
    web-02 = mkHost {
      hostName = "web-02";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec =
        orgDefaults
        // {
          isImpermanent = true;
        };
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
    };

    # edge-01: minimal edge device
    edge-01 = mkHost {
      hostName = "edge-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec =
        orgDefaults
        // {
          isMinimal = true;
        };
    };

    # srv-01: production server
    srv-01 = mkHost {
      hostName = "srv-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec =
        orgDefaults
        // {
          isServer = true;
        };
    };

    # agent-test: exercises agent with tags and health checks
    agent-test = mkHost {
      hostName = "agent-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
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
      hostSpec =
        orgDefaults
        // {
          isServer = true;
        };
      modules = [
        {
          nixfleet.secrets.enable = true;
        }
      ];
    };

    # infra-test: exercises backup + monitoring scopes
    infra-test = mkHost {
      hostName = "infra-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
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
  };
}

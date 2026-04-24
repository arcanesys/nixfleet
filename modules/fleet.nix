# Minimal test fleet for the NixFleet framework repo.
# These hosts exist to make eval tests pass - they are NOT a real org fleet.
# No secrets, no agenix, no real hardware.
# Fleet-specific hostSpec options (isDev, isGraphical, useHyprland, theme, etc.)
# are NOT available here - those are declared by consuming fleets.
#
# Hosts compose via roles from `arcanesys/nixfleet-scopes`:
# - server role: base + operators + firewall + secrets + monitoring + user (wheel)
# - workstation role: base + operators + firewall + secrets + HM + backup + user (groups)
# - endpoint role: base + secrets; no user (distro owns user model)
# - microvm-guest role: base only; no firewall/user (host owns)
{
  config,
  inputs,
  ...
}: let
  mkHost = config.flake.lib.mkHost;

  scopes = inputs.nixfleet-scopes.scopes;

  # Shared organization defaults - just a let binding, no framework function.
  # userName and sshAuthorizedKeys are now owned by the operators scope.
  orgDefaults = {
    timeZone = "UTC";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
  };

  # Shared operators module for hosts using the "deploy" primary user.
  # Placeholder key for eval tests only. Fleet repos set real keys.
  orgOperators = {
    nixfleet.operators = {
      primaryUser = "deploy";
      rootSshKeys = ["ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"];
      users.deploy = {
        isAdmin = true;
        sshAuthorizedKeys = [
          "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
        ];
      };
    };
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
        orgOperators
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
        orgOperators
        {nixfleet.impermanence.enable = true;}
      ];
    };

    # dev-01: developer workstation, custom user (alice)
    dev-01 = mkHost {
      hostName = "dev-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        {
          nixfleet.operators = {
            primaryUser = "alice";
            rootSshKeys = ["ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"];
            users.alice = {
              isAdmin = true;
              sshAuthorizedKeys = ["ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"];
            };
          };
        }
      ];
    };

    # edge-01: minimal edge device (no role - just mkHost mechanism).
    # Represents a "bare" host: gets core/_nixos (nix settings, openssh,
    # root key) but no scope opinions. Operators scope not active here,
    # so userName must be set explicitly.
    edge-01 = mkHost {
      hostName = "edge-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults // {userName = "deploy";};
    };

    # srv-01: production server
    srv-01 = mkHost {
      hostName = "srv-01";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.server
        orgOperators
      ];
    };

    # agent-test: exercises the v0.2 agent against a stub CP.
    agent-test = mkHost {
      hostName = "agent-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        orgOperators
        {
          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp.test:8080";
          };
          nixfleet.trust.ciReleaseKey.current = {
            algorithm = "ed25519";
            public = "AAAA"; # eval-fixture placeholder; real hosts pin real keys
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
        orgOperators
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
        orgOperators
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
        orgOperators
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
        orgOperators
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
        orgOperators
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

  # darwin-agent-test retired alongside the v0.1 darwin agent scope
  # module (#29). Darwin agent support is on the Phase 4 trim list; any
  # future darwin support will be reintroduced on the v0.2 contract.
}

# Quick Start

Define a fleet, deploy your first host, and enable orchestration - all in 15 minutes.

## Prerequisites

- **Nix** with flakes enabled (`experimental-features = nix-command flakes` in `~/.config/nix/nix.conf`)
- **SSH access** to at least one target machine (root login or `nixos-anywhere` compatible)

## 1. Create a Fleet

Create a new directory and initialize a `flake.nix`:

```nix
# flake.nix
{
  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {nixfleet, ...}: {
    nixosConfigurations.web-01 = nixfleet.lib.mkHost {
      hostName = "web-01";
      platform = "x86_64-linux";
      hostSpec = {
        timeZone = "UTC";
      };
      modules = [
        nixfleet.scopes.roles.server
        ./hosts/web-01/hardware-configuration.nix
        ./hosts/web-01/disk-config.nix
        {
          nixfleet.operators = {
            primaryUser = "deploy";
            users.deploy = {
              isAdmin = true;
              sshAuthorizedKeys = [
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... you@workstation"
              ];
            };
          };
        }
      ];
    };

    nixosConfigurations.web-02 = nixfleet.lib.mkHost {
      hostName = "web-02";
      platform = "x86_64-linux";
      hostSpec = {
        timeZone = "UTC";
      };
      modules = [
        nixfleet.scopes.roles.server
        ./hosts/web-02/hardware-configuration.nix
        ./hosts/web-02/disk-config.nix
        {
          nixfleet.operators = {
            primaryUser = "deploy";
            users.deploy = {
              isAdmin = true;
              sshAuthorizedKeys = [
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... you@workstation"
              ];
            };
          };
        }
      ];
    };
  };
}
```

Each call to `mkHost` returns a full `nixosSystem`. The `server` role imports the base, operators, firewall, secrets, monitoring, and impermanence scopes. The operators scope manages user accounts - `primaryUser` is the identity anchor for Home Manager, secrets, and impermanence paths. The framework also injects disko and the fleet agent/control-plane service modules (disabled by default).

> **Tip:** Run `git init && git add -A` before any `nix` command. Flakes only see files tracked by git.

## 2. Deploy the First Host

Use standard NixOS tooling. No custom scripts.

```bash
# Fresh install (wipes disk, installs NixOS)
nixos-anywhere --flake .#web-01 root@192.168.1.10

# Subsequent rebuilds
nixos-rebuild switch --flake .#web-01 --target-host root@192.168.1.10
```

Repeat for `web-02`. At this point you have two independently managed NixOS machines. Everything below is optional.

## 3. Enable Fleet Orchestration

Add the control plane to `web-01` and the fleet agent to both hosts. Create a shared module:

```nix
# modules/fleet-agent.nix
{config, ...}: {
  services.nixfleet-agent = {
    enable = true;
    controlPlaneUrl = "http://web-01:8080";
    tags = ["web"];
    healthChecks.http = [
      {
        url = "http://localhost:80/health";
        interval = 5;
        timeout = 3;
        expectedStatus = 200;
      }
    ];
  };
}
```

Then add the control plane to `web-01`:

```nix
# modules/control-plane.nix
{
  services.nixfleet-control-plane = {
    enable = true;
    listen = "0.0.0.0:8080";
    openFirewall = true;
  };
}
```

Extract the operators config into a shared module so both hosts use the same user definition:

```nix
# modules/operators.nix
{
  nixfleet.operators = {
    primaryUser = "deploy";
    users.deploy = {
      isAdmin = true;
      sshAuthorizedKeys = [
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... you@workstation"
      ];
    };
  };
}
```

Include all modules in your `mkHost` calls:

```nix
nixosConfigurations.web-01 = nixfleet.lib.mkHost {
  hostName = "web-01";
  platform = "x86_64-linux";
  modules = [
    nixfleet.scopes.roles.server
    ./hosts/web-01/hardware-configuration.nix
    ./hosts/web-01/disk-config.nix
    ./modules/fleet-agent.nix
    ./modules/control-plane.nix
    ./modules/operators.nix
  ];
};

nixosConfigurations.web-02 = nixfleet.lib.mkHost {
  hostName = "web-02";
  platform = "x86_64-linux";
  modules = [
    nixfleet.scopes.roles.server
    ./hosts/web-02/hardware-configuration.nix
    ./hosts/web-02/disk-config.nix
    ./modules/fleet-agent.nix
    ./modules/operators.nix
  ];
};
```

Rebuild both hosts to activate the agent and control plane.

## 4. Deploy to the Fleet

First-time setup - create a config file and bootstrap the admin API key:

```bash
nixfleet init \
  --control-plane-url https://cp.example.com:8080 \
  --ca-cert ./fleet-ca.pem \
  --cache-url http://cache.example.com:5000 \
  --push-to ssh://root@cache.example.com
nixfleet bootstrap
```

This writes `.nixfleet.toml` to the repo and saves the API key to `~/.config/nixfleet/credentials.toml`. Subsequent commands run with no flags.

Now deploy - the one-command form builds all targeted hosts, pushes them to the cache, registers a release, and triggers a canary rollout:

```bash
nixfleet deploy --push-to ssh://root@cache.example.com --tags web --strategy canary --wait
```

Or split it into explicit steps if you want to inspect or replay the release:

```bash
nixfleet release create --push-to ssh://root@cache.example.com
# Output: Release rel-abc123 created (2 hosts)
nixfleet deploy --release rel-abc123 --tags web --strategy canary --wait
```

The `--strategy` flag controls rollout behavior:
- `all-at-once` - deploy to every matching host simultaneously (default)
- `canary` - deploy to one host first, verify health, then continue
- `staged` - deploy in configurable batch sizes (`--batch-size 1,25%,100%`)

The agent checks health (`http://localhost:80/health`) after each switch. On failure, it automatically rolls back to the previous generation. The control plane verifies each machine reports its NEW `current_generation` before accepting health as proof of successful deployment.

## 5. Check Fleet Status

```bash
nixfleet status
nixfleet status --json
```

## Next Steps

- [Design Guarantees](design-guarantees.md) - properties that hold across every NixFleet deployment
- [Installation](installation.md) - detailed install methods, ISO builds, troubleshooting
- [Rollouts](../deploying/rollouts.md) - batch sizes, failure thresholds, pause/resume
- [The mkHost API](../defining-hosts/mkhost.md) - all parameters and what the framework injects

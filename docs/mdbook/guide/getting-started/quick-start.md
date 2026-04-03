# Quick Start

Define a fleet, deploy your first host, and enable orchestration — all in 15 minutes.

## Prerequisites

- **Nix** with flakes enabled (`experimental-features = nix-command flakes` in `~/.config/nix/nix.conf`)
- **SSH access** to at least one target machine (root login or `nixos-anywhere` compatible)

## 1. Create a Fleet

Create a new directory and initialize a `flake.nix`:

```nix
# flake.nix
{
  inputs = {
    nixfleet.url = "github:your-org/nixfleet";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {nixfleet, ...}: {
    nixosConfigurations.web-01 = nixfleet.lib.mkHost {
      hostName = "web-01";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "deploy";
        timeZone = "UTC";
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... you@workstation"
        ];
      };
      modules = [
        ./hosts/web-01/hardware-configuration.nix
        ./hosts/web-01/disk-config.nix
      ];
    };

    nixosConfigurations.web-02 = nixfleet.lib.mkHost {
      hostName = "web-02";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "deploy";
        timeZone = "UTC";
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... you@workstation"
        ];
      };
      modules = [
        ./hosts/web-02/hardware-configuration.nix
        ./hosts/web-02/disk-config.nix
      ];
    };
  };
}
```

Each call to `mkHost` returns a full `nixosSystem`. The framework injects core modules, Home Manager, disko, impermanence support, and the fleet agent/control-plane service modules (disabled by default).

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

Include these modules in your `mkHost` calls:

```nix
nixosConfigurations.web-01 = nixfleet.lib.mkHost {
  hostName = "web-01";
  platform = "x86_64-linux";
  hostSpec = { userName = "deploy"; };
  modules = [
    ./hosts/web-01/hardware-configuration.nix
    ./hosts/web-01/disk-config.nix
    ./modules/fleet-agent.nix
    ./modules/control-plane.nix
  ];
};

nixosConfigurations.web-02 = nixfleet.lib.mkHost {
  hostName = "web-02";
  platform = "x86_64-linux";
  hostSpec = { userName = "deploy"; };
  modules = [
    ./hosts/web-02/hardware-configuration.nix
    ./hosts/web-02/disk-config.nix
    ./modules/fleet-agent.nix
  ];
};
```

Rebuild both hosts to activate the agent and control plane.

## 4. Deploy to the Fleet

With orchestration enabled, use the NixFleet CLI:

```bash
# Build and deploy to all hosts tagged "web", canary strategy
nixfleet deploy --tag web --strategy canary --generation /nix/store/abc123... --wait
```

The `--strategy` flag controls rollout behavior:
- `all-at-once` — deploy to every matching host simultaneously (default)
- `canary` — deploy to one host first, verify health, then continue
- `staged` — deploy in configurable batch sizes (`--batch-size 1,25%,100%`)

The agent checks health (`http://localhost:80/health`) after each switch. On failure, it automatically rolls back to the previous generation.

## 5. Check Fleet Status

```bash
nixfleet status
nixfleet status --json
```

## Next Steps

- [Design Guarantees](design-guarantees.md) — properties that hold across every NixFleet deployment
- [Installation](installation.md) — detailed install methods, ISO builds, troubleshooting
- [Rollouts](../deploying/rollouts.md) — batch sizes, failure thresholds, pause/resume
- [The mkHost API](../defining-hosts/mkhost.md) — all parameters and what the framework injects

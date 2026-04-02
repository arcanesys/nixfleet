# Phase 3: Framework Infrastructure — Pre-Spec

**Status:** Vision document — not a design spec. Details will be refined into proper specs when work begins.
**Date:** 2026-04-02
**Depends on:** Phase 2 (open source launch) complete

## Goal

Make nixfleet a self-sufficient infrastructure platform. A fleet operator should be able to go from "bare metal" to "fully managed fleet with binary cache, microVMs, and automated deployments" using only nixfleet modules — no external tooling or manual setup.

## Envisioned Modules

### 1. Attic Binary Cache

**What:** NixOS module wrapping `services.atticd` for the cache server + client-side module configuring `nix.settings.substituters`.

**Why:** Every fleet needs a binary cache. Building the same derivation on 50 machines wastes time and bandwidth. Attic is the community standard for self-hosted Nix caches.

**Rough shape:**

```nix
# Server (on a dedicated host or the CP host)
services.nixfleet-attic-server = {
  enable = true;
  domain = "cache.fleet.example.com";
  # Agenix-encrypted token for push access
  tokenFile = config.age.secrets.attic-token.path;
  storage = {
    type = "local";  # or "s3"
    path = "/var/lib/attic";
  };
};

# Client (on all fleet hosts)
services.nixfleet-attic-client = {
  enable = true;
  cacheUrl = "https://cache.fleet.example.com";
  publicKey = "cache.fleet.example.com:AAAA...";
};
```

**Integration with rollouts:** The rollout executor could push closures to Attic before starting a rollout, then agents pull from Attic (faster than pulling from the builder). The `cache_url` field on `CreateRolloutRequest` was designed for this.

**Open questions:**
- Should the server module be in nixfleet or a separate flake?
- Post-build hook for automatic push — where does it live?
- Garbage collection policy — per-host or fleet-wide?

### 2. MicroVM Host

**What:** NixOS module wrapping `microvm.host` for running lightweight VMs (microVMs) on fleet hosts.

**Why:** Isolated services, CI runners, ephemeral test environments. MicroVMs are faster than containers for NixOS workloads (full NixOS in ~100ms boot).

**Rough shape:**

```nix
services.nixfleet-microvm-host = {
  enable = true;
  vms = {
    ci-runner = {
      flake = "github:your-org/ci-runner-vm";
      vcpu = 2;
      mem = 2048;
      network = {
        type = "tap";
        bridge = "br0";
      };
      volumes = [{
        source = "/var/lib/ci-runner";
        target = "/data";
        type = "virtiofs";
      }];
    };
  };
};
```

**Integration with fleet:** MicroVMs could be managed as fleet members — each gets a nixfleet agent, reports to the CP, participates in rollouts. This creates a recursive management model (host manages VMs, VMs are fleet members).

**Open questions:**
- TAP networking defaults vs bridge config
- Resource limits enforcement (cgroups, memory balloon)
- Should VMs auto-register with the CP?
- Hot migration between hosts (aspirational)

### 3. Rollout Strategies — Advanced

**What:** Extensions to the rollout system built in Phase 1.

**Envisioned features:**

- **CP-side rollout defaults per tag group** ("production always uses canary") — the "path to B" from the fleet orchestration design. Requires a `policies` table + CRUD endpoints + CLI `policy set/list`.
- **Scheduled rollouts** — "deploy at 02:00 UTC" via `scheduled_at` field on rollout creation. The executor checks the timestamp before starting.
- **Webhooks** on rollout state changes — notify Slack, PagerDuty, or custom URLs when a rollout pauses, fails, or completes. Configurable per tag group or globally.
- **Rollout history dashboard data** — enrich the `GET /api/v1/rollouts` response with duration, success rate, and machine-level timelines for dashboard consumption.

### 4. Backup Module

**What:** Declarative backup with sane defaults, wrapping restic or borgbackup.

**Why:** Every fleet needs backups. Nobody wants to wire restic + systemd timers + retention + health monitoring from scratch.

**Rough shape:**

```nix
services.nixfleet-backup = {
  enable = true;
  backend = "restic";  # or "borgbackup"
  paths = ["/persist"];
  repository = "s3:s3.amazonaws.com/fleet-backups/${config.networking.hostName}";
  passwordFile = config.age.secrets.restic-password.path;
  schedule = "daily";
  retention = {
    daily = 7;
    weekly = 4;
    monthly = 6;
  };
  healthCheck.url = "https://hc-ping.com/xxx";  # ping on success
};
```

**Integration with agent:** The agent could report backup status (last successful, last failure, repository size) to the CP alongside health checks. The compliance reporting module (Phase 4) would consume this.

**Open questions:**
- Should the module handle repository initialization?
- Restore workflow — module-assisted or manual?
- Encryption key management — agenix, sops, or module-managed?

### 5. Firewall Baseline

**What:** A scope module that provides sane firewall defaults for all fleet hosts.

**Why:** Every host needs a firewall. Forgetting to enable it is a common mistake. The agent and CP ports should open automatically when their services are enabled.

**Rough shape:**

```nix
# Auto-enabled on non-minimal hosts (scope pattern)
config = lib.mkIf (!hS.isMinimal) {
  networking.firewall = {
    enable = true;
    allowedTCPPorts = lib.optionals config.services.openssh.enable [22]
      ++ lib.optionals config.services.nixfleet-control-plane.enable [
        config.services.nixfleet-control-plane.port
      ];
  };
};
```

**Open questions:**
- Should this be a framework scope (always included) or opt-in?
- Rate limiting on SSH? (fail2ban integration)
- Outbound filtering? (probably too opinionated for framework)

## Priority Order

Based on the discussion of 2026-04-02:

1. **Attic** — highest value, enables fast rollouts
2. **MicroVM** — enables isolated services and CI
3. **Rollout strategies advanced** — builds on Phase 1 foundation
4. **Backup module** — every fleet needs it
5. **Firewall baseline** — simple but important default

Each module gets its own design spec → implementation plan → PR cycle. They are independent and can be built in parallel by different contributors.

## Decision Framework

When evaluating whether a module belongs in nixfleet or in a fleet repo:

> **"Would a stranger with 10 machines need this?"**
> If yes → framework. If no → fleet-specific.

Nixfleet provides **infrastructure primitives and fleet orchestration**. Fleet repos provide **opinions about what runs on the machines**.

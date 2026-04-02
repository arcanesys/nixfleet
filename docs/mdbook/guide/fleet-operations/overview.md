# Fleet Operations

Moving beyond single-host rebuilds to fleet-wide orchestration.

## When You Need This

Standard NixOS commands (`nixos-rebuild`, `nixos-anywhere`) work well for individual machines. But when you manage multiple hosts, you need:

- **Targeted deployments** — deploy to a subset of machines (e.g., all web servers)
- **Safe rollouts** — deploy incrementally with automatic pause on failure
- **Health verification** — confirm machines are healthy after deployment
- **Centralized visibility** — see fleet status from one place

NixFleet's agent and control plane provide this layer on top of standard NixOS tooling.

## Architecture

```
┌─────────────┐       ┌──────────────────┐
│  nixfleet   │──────▶│  Control Plane    │
│  CLI        │  API  │  (registry,       │
└─────────────┘       │   rollouts,       │
                      │   audit log)      │
                      └────────┬─────────┘
                               │ poll
              ┌────────────────┼────────────────┐
              │                │                │
        ┌─────▼─────┐   ┌─────▼─────┐   ┌─────▼─────┐
        │  Agent     │   │  Agent     │   │  Agent     │
        │  web-01    │   │  web-02    │   │  db-01     │
        └───────────┘   └───────────┘   └───────────┘
```

- **Agent** — runs on each managed host, polls the control plane, applies generations, runs health checks, reports status
- **Control Plane** — HTTP server that maintains the machine registry, orchestrates rollouts, and provides the API
- **CLI** — operator tool that talks to the control plane

## Getting Started

1. [Setting Up the Control Plane](control-plane-setup.md) — deploy the CP on a host
2. [Enrolling Agents](agent-enrollment.md) — connect hosts to the control plane
3. [Deploying to Your Fleet](deploying.md) — rollouts, strategies, and health checks

## Without the Control Plane

The agent and control plane are optional. You can use NixFleet purely as a configuration framework with `mkHost` and deploy via standard `nixos-rebuild` commands. The fleet orchestration layer is additive — enable it when your fleet grows beyond what manual rebuilds can handle.

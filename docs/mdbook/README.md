# NixFleet

Declarative NixOS fleet management with staged rollouts and automatic rollback.

NixFleet combines a thin configuration framework (`mkHost`) with an optional orchestration layer (agent + control plane) for fleet-wide deployments. The framework builds on standard NixOS tooling - it doesn't replace `nixos-rebuild` or `nixos-anywhere`, it adds reproducible multi-host configuration and health-driven deployment safety on top.

Start with the [Quick Start](guide/getting-started/quick-start.md).

# Crates

NixFleet's Rust workspace consists of 8 crates with clear separation of concerns. Each file in this directory is a one-screen mental model for one crate; for type signatures and method-level detail, follow the rustdoc link.

| Crate | One-line summary |
|-------|------------------|
| [nixfleet-proto](nixfleet-proto.md) | Wire types: Serde-derived schema for every artefact and HTTP body. |
| [nixfleet-canonicalize](nixfleet-canonicalize.md) | JCS canonical JSON for signing - lean deps, no async runtime. |
| [nixfleet-verify-artifact](nixfleet-verify-artifact.md) | Offline auditor: verifies signed artefacts against trust roots. |
| [nixfleet-reconciler](nixfleet-reconciler.md) | Pure decision procedure: reconcile, verify_artifact, state machines. |
| [nixfleet-release](nixfleet-release.md) | CI release tool: signs fleet.resolved.json + revocations sidecar. |
| [nixfleet-cli](nixfleet-cli.md) | Operator umbrella binary (`nixfleet` subcommands). |
| [nixfleet-agent](nixfleet-agent.md) | Host daemon: polls CP, fetches/applies closures, reports back. |
| [nixfleet-control-plane](nixfleet-control-plane.md) | Axum HTTP service + SQLite; routes signed intent to agents. |

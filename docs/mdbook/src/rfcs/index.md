# RFCs

Authoritative design documents for the v0.2+ contract. Each RFC owns one boundary; together they define what is load-bearing across releases.

| RFC | Topic |
|-----|-------|
| [RFC-0001](0001-fleet-nix.md) | Declarative fleet topology (`mkFleet`, selectors, rollouts) |
| [RFC-0002](0002-reconciler.md) | Reconciler decision procedure |
| [RFC-0003](0003-protocol.md) | Agent / control-plane wire protocol |
| [RFC-0004](0004-hardware-rooted-trust.md) | Hardware-rooted trust (TPM, attestation) |
| [RFC-0005](0005-trust-lifecycle.md) | Trust lifecycle (operator roles, rotation) |
| [RFC-0006](0006-freshness-window-policy.md) | Freshness-window policy |
| [RFC-0007](0007-air-gapped-operation.md) | Air-gapped operation (signed bundles) |

The RFC pages above are mdbook wrappers that include the canonical sources from the repo's `docs/rfcs/` tree.

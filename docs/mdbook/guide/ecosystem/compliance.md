# Compliance

NixFleet's compliance layer ships in the [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) companion repository - a standalone collection of regulatory controls, framework presets, and evidence probes for NixOS hosts.

Each control enforces a security measure and produces machine-readable evidence via probes. Evidence is collected on a schedule and written to `/var/lib/nixfleet-compliance/evidence.json`. The governance engine lets fleet operators set enforcement levels, host-type scoping, and per-rule exceptions with mandatory rationale.

> **Repository:** [github.com/arcanesys/nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) - MIT licensed, works standalone or alongside nixfleet and nixfleet-scopes.

## Quick Start

```nix
{
  inputs.compliance.url = "github:arcanesys/nixfleet-compliance";
  # In your mkHost modules:
  modules = [
    compliance.nixosModules.nis2
    {
      compliance.frameworks.nis2 = {
        enable = true;
        entityType = "essential";
      };
    }
  ];
}
```

## Frameworks

| Framework | Regulation | Controls | Differentiation |
|-----------|-----------|----------|-----------------|
| NIS2 | Directive 2022/2555 | 12 | essential vs important |
| DORA | Regulation 2022/2554 | 9 | critical provider vs standard |
| ISO 27001 | ISO/IEC 27001:2022 | 14 | full vs partial scope |
| ANSSI | BP-028 v2.0 | 7 | minimal / intermediary / reinforced / high |

## Controls

| Control | What it enforces |
|---------|-----------------|
| access-control | SSH key-only auth, root login disabled, idle session timeout |
| asset-inventory | Host, service, and network inventory from running system |
| audit-logging | Journald persistence, auditd with execve tracking, log retention |
| authentication | MFA policy, PAM modules, SSH certificate auth |
| backup-retention | Backup service verification, last backup age, retention compliance |
| baseline-hardening | Kernel sysctl, IOMMU, filesystem permissions (ANSSI R7-R14) |
| change-management | System rebuild freshness, generation frequency |
| disaster-recovery | Generation retention, RTO target, recovery test interval |
| encryption-at-rest | LUKS verification, encrypted swap, tmpfs /tmp |
| encryption-in-transit | TLS minimum version, certificate inventory and expiry |
| incident-response | Rollback readiness, journal availability, alert retention |
| key-management | SSH host key age and algorithm, LUKS key slots, rotation policy |
| network-segmentation | Firewall status, VLAN detection, interface inventory |
| secure-boot | EFI support, secure boot status, signed unified kernel images |
| supply-chain | flake.lock pinning, SBOM generation, nixpkgs staleness |
| vulnerability-mgmt | Nixpkgs freshness, scan interval, critical vulnerability blocking |

## Governance

| Option | Values | Description |
|--------|--------|-------------|
| `enforceMode` | enforce, report | Enforce applies NixOS config and runs probes; report only runs probes |
| `level` | minimal, standard, strict, paranoid | Rules above this severity threshold are auto-disabled |
| `hostType` | server, workstation, appliance | Rules scope themselves to matching host types |
| `excludes` | list of tags | Tag-based rule exclusions (e.g., `["no-ipv6"]`) |
| `exceptions` | attrs with rationale | Per-rule exceptions with mandatory reason, included in audit report |

```nix
compliance.governance = {
  enforceMode = "enforce";
  level = "standard";
  hostType = "server";
  exceptions.BH-07 = {
    rationale = "IPv6 required for internal mesh networking";
  };
};
```

## Evidence Collection

Probes run on a configurable schedule - hourly for essential/critical entities, daily for important/standard - and produce JSON. The `compliance-check` CLI runs all probes interactively:

```bash
compliance-check              # colored summary
VERBOSE=1 compliance-check    # detailed JSON per control
```

## Framework Mappings

For detailed article-by-article regulatory mappings:

- [NIS2 Article 21 mapping](https://github.com/arcanesys/nixfleet-compliance/blob/main/docs/nis2-mapping.md)
- [ISO 27001 Annex A mapping](https://github.com/arcanesys/nixfleet-compliance/blob/main/docs/iso27001-mapping.md)
- [DORA Chapter III mapping](https://github.com/arcanesys/nixfleet-compliance/blob/main/docs/dora-mapping.md)

## NixOS Advantage

NixOS provides unique compliance properties. `flake.lock` is a cryptographically verifiable supply chain manifest - every input is pinned by hash. Content-addressing makes binary tampering detectable. Impermanence prevents malware persistence by wiping the root filesystem on every reboot. Declarative configuration means the audit configuration IS the actual running configuration - there is no drift between what was approved and what is deployed.

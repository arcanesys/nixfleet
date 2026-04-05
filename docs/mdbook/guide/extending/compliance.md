# Compliance

[nixfleet-compliance](https://github.com/abstracts33d/nixfleet-compliance) provides regulatory compliance controls as NixOS modules. It works standalone or alongside nixfleet — when consumed via `mkHost` on an impermanent system, it automatically persists evidence across reboots.

```nix
# Add to your fleet flake.nix inputs:
inputs.compliance.url = "github:abstracts33d/nixfleet-compliance";

# In mkHost modules:
modules = [
  compliance.nixosModules.nis2
  { compliance.frameworks.nis2.enable = true;
    compliance.frameworks.nis2.entityType = "essential"; }
];
```

This enables 12 NIS2 controls covering all Article 21 requirements. Evidence is collected hourly and written to `/var/lib/nixfleet-compliance/evidence.json`.

See the [nixfleet-compliance README](https://github.com/abstracts33d/nixfleet-compliance) for individual controls, framework options, and evidence format.

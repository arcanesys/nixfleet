# MicroVM Host Options

All options under `services.nixfleet-microvm-host`. The module is auto-included by mkHost and disabled by default. Enable with `services.nixfleet-microvm-host.enable = true`.

The module imports the upstream `microvm.nixosModules.host` module. MicroVMs themselves are defined via the standard `microvm.vms` option from the microvm.nix framework; this module only provides the bridge networking, DHCP, and NAT infrastructure for the host.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet MicroVM host. |
| `bridge.name` | `str` | `"nixfleet-br0"` | Bridge interface name for microVM networking. |
| `bridge.address` | `str` | `"10.42.0.1/24"` | Bridge IP address with CIDR prefix. |
| `dhcp.enable` | `bool` | `true` | Run a dnsmasq DHCP server on the bridge. |
| `dhcp.range` | `str` | `"10.42.0.10,10.42.0.254,1h"` | DHCP range in dnsmasq format (`start,end,lease-time`). |

## What the module configures

When enabled, the module:

- Creates a systemd-networkd bridge interface (`bridge.name`) with the given IP address.
- Enables `net.ipv4.ip_forward` for NAT.
- Configures `networking.nat` with the bridge as an internal interface so microVMs can reach the outside.
- Optionally starts dnsmasq on the bridge with the configured DHCP range and the bridge IP as the default router.

## Impermanence

On impermanent hosts (`nixfleet.impermanence.enable = true`), the module automatically persists `/var/lib/microvms` across reboots.

## Example

```nix
services.nixfleet-microvm-host = {
  enable = true;
  bridge.address = "10.42.0.1/24";
  dhcp.range = "10.42.0.10,10.42.0.100,12h";
};

# Define a microVM using the upstream microvm.nix API
microvm.vms.my-vm = {
  config = { ... };
};
```

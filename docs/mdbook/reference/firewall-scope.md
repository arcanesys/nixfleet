# Firewall Scope

> This module is provided by [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). It is documented here as part of the NixFleet ecosystem reference.

The firewall scope applies SSH rate limiting, connection drop logging, and the nftables backend to all non-minimal hosts. It has no user-configurable options.

## Activation

The scope activates when `nixfleet.firewall.enable = true`. Roles like `server` and `workstation` set this automatically. Minimal roles (`endpoint`, `microvm-guest`) leave it disabled by default.

## What it provides

**nftables backend**

Sets `networking.nftables.enable = true`. This is the forward-compatible choice: Linux 6.17+ drops the `ip_tables` kernel module. Fleet repos using `networking.firewall.extraCommands` (iptables syntax) will receive an assertion failure at evaluation time, forcing migration before the kernel forces it.

**SSH rate limiting**

Adds nftables input rules that accept at most 5 new SSH connections per minute per source IP and drop the rest:

```
tcp dport 22 ct state new limit rate 5/minute accept
tcp dport 22 ct state new drop
```

This limits brute-force attempts without blocking legitimate access.

**Drop logging**

Enables `networking.firewall.logRefusedConnections` and `networking.firewall.logReversePathDrops`. Dropped packets appear in the system journal under `kernel`, making it straightforward to diagnose connectivity issues and detect port scans.

## No user-configurable options

The firewall scope is intentionally opinionated. These settings are appropriate for any production NixOS host and require no per-host tuning. Fleet repos needing custom firewall rules add them via standard NixOS options (`networking.firewall.extraInputRules`, `networking.firewall.allowedTCPPorts`, etc.) alongside the scope.

# Control Plane Options

All options under `services.nixfleet-control-plane`. The module is auto-included by mkHost and disabled by default.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet control plane server. |
| `listen` | `str` | `"0.0.0.0:8080"` | Address and port to listen on. |
| `dbPath` | `str` | `"/var/lib/nixfleet-cp/state.db"` | Path to the SQLite state database. |
| `openFirewall` | `bool` | `false` | Open the control plane port in the firewall. The port is parsed from the `listen` value. |

## Systemd service

| Setting | Value |
|---------|-------|
| Target | `multi-user.target` |
| After | `network-online.target` |
| Restart | `always` (10s delay) |
| StateDirectory | `nixfleet-cp` |
| NoNewPrivileges | `true` |
| ProtectHome | `true` |
| PrivateTmp | `true` |
| PrivateDevices | `true` |
| ProtectKernelTunables | `true` |
| ProtectKernelModules | `true` |
| ProtectControlGroups | `true` |
| ReadWritePaths | `/var/lib/nixfleet-cp` |

## Example

```nix
services.nixfleet-control-plane = {
  enable = true;
  listen = "0.0.0.0:8080";
  openFirewall = true;
};
```

On impermanent hosts, `/var/lib/nixfleet-cp` is automatically persisted.

# Setting Up the Control Plane

The control plane is a NixOS service module. Enable it on one host in your fleet.

## Enable the Service

```nix
# In your host's modules or fleet-wide config:
services.nixfleet-control-plane = {
  enable = true;
  listen = "0.0.0.0:8443";
  openFirewall = true;
};
```

The control plane stores its state in a SQLite database at `/var/lib/nixfleet-cp/state.db`. On impermanent hosts, this path is automatically persisted.

## Verify

After rebuilding:

```sh
# Check the service is running
systemctl status nixfleet-control-plane

# Test the API
curl http://localhost:8443/api/machines
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Enable the control plane |
| `listen` | str | `0.0.0.0:8080` | Listen address and port |
| `dbPath` | str | `/var/lib/nixfleet-cp/state.db` | SQLite database path |
| `openFirewall` | bool | false | Open the listen port |

## Security Considerations

The control plane handles fleet deployment operations. In production:

- Use TLS (configure a reverse proxy or the built-in TLS support)
- Set API keys for authentication (`--api-key` flag on the CLI)
- Restrict network access to trusted hosts

## Next Steps

- [Enrolling Agents](agent-enrollment.md) — connect hosts to the control plane

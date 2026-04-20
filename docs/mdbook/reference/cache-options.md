# Cache Options

Options for `services.nixfleet-cache-server` and `services.nixfleet-cache`. Both modules are auto-included by mkHost and disabled by default.

The cache server uses [harmonia](https://github.com/nix-community/harmonia), which serves paths directly from the local Nix store over HTTP. No separate storage backend, database, or push protocol is needed.

## services.nixfleet-cache-server

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet binary cache server (harmonia). |
| `port` | `port` | `5000` | Port to listen on. |
| `openFirewall` | `bool` | `false` | Open the cache server port in the firewall. |
| `signingKeyFile` | `str` | - (required) | Path to the Nix signing key file for on-the-fly signing. Must be readable by the `harmonia` user (set `age.secrets.<name>.owner = "harmonia"` with agenix, or `sops.secrets.<name>.owner = "harmonia"` with sops-nix). Example: `"/run/secrets/cache-signing-key"`. |

## services.nixfleet-cache

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet binary cache client. |
| `cacheUrl` | `str` | - (required) | URL of the binary cache server. Example: `"https://cache.fleet.example.com"`. |
| `publicKey` | `str` | - (required) | Cache signing public key in `name:base64` format. Example: `"cache.fleet.example.com:AAAA...="`.|

## Systemd service (server)

| Setting | Value |
|---------|-------|
| Unit | `nixfleet-cache-server.service` |
| WantedBy | `multi-user.target` |
| After | `network-online.target`, `nix-daemon.service` |
| Restart | `always` (10s delay) |
| NoNewPrivileges | `true` |
| ProtectHome | `true` |
| PrivateTmp | `true` |
| PrivateDevices | `true` |
| ProtectKernelTunables | `true` |
| ProtectKernelModules | `true` |
| ProtectControlGroups | `true` |

Harmonia is stateless - it serves directly from the local Nix store. No state directory or persistence configuration is needed.

## Using a different cache backend

Fleet repos that need Attic, Cachix, or another cache backend can add them as their own flake input and configure them via plain NixOS modules. The `--push-hook` CLI flag supports custom push commands for any backend.

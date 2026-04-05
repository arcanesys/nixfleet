# Attic Options

Options for `services.nixfleet-attic-server` and `services.nixfleet-attic-client`. Both modules are auto-included by mkHost and disabled by default.

> **Note:** The `attic` input is currently pinned to `github:booxter/attic/newer-nix` for NixOS 2.31 compatibility. Issue [#22](https://github.com/your-org/nixfleet/issues/22) tracks reverting to upstream once Attic PR #300 merges.

## services.nixfleet-attic-server

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet Attic binary cache server. |
| `listen` | `str` | `"0.0.0.0:8081"` | Address and port to listen on. |
| `openFirewall` | `bool` | `false` | Open the Attic server port in the firewall. |
| `dbPath` | `str` | `"/var/lib/nixfleet-attic/server.db"` | Path to the SQLite database. |
| `signingKeyFile` | `str` | — (required) | Path to the cache signing key file. Example: `"/run/secrets/attic-signing-key"`. |
| `storage.type` | `enum ["local" "s3"]` | `"local"` | Storage backend type. |
| `storage.local.path` | `str` | `"/var/lib/nixfleet-attic/storage"` | Local filesystem storage path. |
| `storage.s3.bucket` | `str` | `""` | S3 bucket name. |
| `storage.s3.region` | `str` | `""` | S3 region. |
| `storage.s3.endpoint` | `nullOr str` | `null` | S3-compatible endpoint URL (e.g. MinIO). |
| `garbageCollection.schedule` | `str` | `"weekly"` | Systemd calendar expression for garbage collection. |
| `garbageCollection.keepSinceLastPush` | `str` | `"90 days"` | Duration to keep paths after last push. |

## services.nixfleet-attic-client

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the NixFleet Attic binary cache client. |
| `cacheUrl` | `str` | — (required) | URL of the Attic cache server. Example: `"https://cache.fleet.example.com"`. |
| `publicKey` | `str` | — (required) | Cache signing public key in `name:base64` format. Example: `"cache.fleet.example.com:AAAA...="`.|

## Systemd service (server)

| Setting | Value |
|---------|-------|
| Unit | `nixfleet-attic-server.service` |
| WantedBy | `multi-user.target` |
| After | `network-online.target` |
| Restart | `always` (10s delay) |
| StateDirectory | `nixfleet-attic` |
| NoNewPrivileges | `true` |
| ProtectHome | `true` |
| PrivateTmp | `true` |
| PrivateDevices | `true` |
| ProtectKernelTunables | `true` |
| ProtectKernelModules | `true` |
| ProtectControlGroups | `true` |
| ReadWritePaths | `/var/lib/nixfleet-attic` |

A garbage collection timer (`nixfleet-attic-gc.timer`) runs on `garbageCollection.schedule` with a 1-hour randomized delay.

## Impermanence

On impermanent hosts (`hostSpec.isImpermanent = true`), the server module automatically persists `/var/lib/nixfleet-attic` across reboots.

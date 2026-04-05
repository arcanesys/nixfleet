# Binary Cache (Attic)

Attic is a self-hosted binary cache server backed by the Nix store signing protocol. Running a fleet cache means agents fetch closures from your own infrastructure instead of rebuilding or pulling from cache.nixos.org on every deploy.

> **Current limitation:** The `attic` flake input is pinned to `github:booxter/attic/newer-nix` for NixOS 2.31 compatibility. Issue [#22](https://github.com/your-org/nixfleet/issues/22) tracks reverting to upstream once Attic PR #300 merges.

## Server setup

Enable the Attic server on a dedicated cache host (or any always-on fleet member):

```nix
services.nixfleet-attic-server = {
  enable = true;
  listen = "0.0.0.0:8081";
  openFirewall = true;
  signingKeyFile = "/run/secrets/attic-signing-key";

  # Local storage (default)
  storage.type = "local";
  storage.local.path = "/var/lib/nixfleet-attic/storage";

  garbageCollection = {
    schedule = "weekly";
    keepSinceLastPush = "90 days";
  };
};
```

### S3 storage

For larger fleets, use an S3-compatible backend:

```nix
services.nixfleet-attic-server = {
  enable = true;
  signingKeyFile = "/run/secrets/attic-signing-key";
  storage = {
    type = "s3";
    s3 = {
      bucket = "my-fleet-cache";
      region = "eu-west-1";
      # endpoint = "https://minio.internal";  # for S3-compatible stores
    };
  };
};
```

### Generating a signing key

```sh
nix-store --generate-binary-cache-key cache.fleet.example.com secret-key.pem public-key.pem
```

Store `secret-key.pem` as an encrypted secret (agenix/sops). Note the `public-key.pem` contents — clients need it.

## Client setup

Enable on agent hosts to configure Nix substituters and install the `attic` CLI:

```nix
services.nixfleet-attic-client = {
  enable = true;
  cacheUrl = "https://cache.fleet.example.com";
  publicKey = "cache.fleet.example.com:AAAA...=";  # contents of public-key.pem
};
```

This adds `cacheUrl` to `nix.settings.substituters` and the public key to `nix.settings.trusted-public-keys`. The `attic` CLI is added to `environment.systemPackages`.

## Pushing to the cache

After a build, push the closure to the cache server:

```sh
# Build
nix build .#nixosConfigurations.web-01.config.system.build.toplevel

# Push to the cache
nix copy --to "https://cache.fleet.example.com?secret-key=/run/secrets/attic-signing-key" \
  ./result
```

Or use the `attic` CLI directly:

```sh
attic login fleet https://cache.fleet.example.com
attic push fleet ./result
```

## Agent fetch workflow

When a deploy is triggered, each agent resolves the closure from substituters in order:

1. `cacheUrl` from `services.nixfleet-attic-client` (or `services.nixfleet-agent.cacheUrl`)
2. Default Nix substituters (`cache.nixos.org`, etc.)

Agents automatically benefit from the fleet cache once the client module is enabled and the signing key is trusted — no additional configuration on the agent side is needed.

To override the cache URL per-deploy from the CLI:

```sh
nixfleet deploy --tag web --generation 42 --cache-url https://cache.fleet.example.com
```

## See also

- [Attic Options](../../reference/attic-options.md) — full option reference
- [Secrets](../extending/secrets.md) — managing the signing key with agenix

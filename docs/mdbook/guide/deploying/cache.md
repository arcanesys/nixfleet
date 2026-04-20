# Binary Cache

A fleet binary cache means agents fetch closures from your own infrastructure instead of rebuilding or pulling from cache.nixos.org on every deploy.

NixFleet ships with [harmonia](https://github.com/nix-community/harmonia) as the default cache server. Harmonia serves paths directly from the local Nix store over HTTP - no separate storage backend, database, or push protocol. Paths are signed on-the-fly using the host's Nix signing key.

## Server setup

Enable the cache server on a dedicated host (or any always-on fleet member):

```nix
services.nixfleet-cache-server = {
  enable = true;
  port = 5000;            # default
  openFirewall = true;
  signingKeyFile = "/run/secrets/cache-signing-key";
};
```

### Generating a signing key

```sh
nix-store --generate-binary-cache-key cache.fleet.example.com secret-key.pem public-key.pem
```

Store `secret-key.pem` as an encrypted secret (agenix/sops). Note the `public-key.pem` contents - clients need it.

### Populating the cache

Harmonia serves whatever is in the local Nix store. To populate it, copy closures to the cache host after building:

```sh
# Push closures to the cache host's Nix store
nixfleet release create --push-to ssh://root@cache.fleet.example.com

# Or with nix copy directly
nix copy --to ssh://root@cache.fleet.example.com /nix/store/...
```

## Client setup

Enable on agent hosts to configure Nix substituters:

```nix
services.nixfleet-cache = {
  enable = true;
  cacheUrl = "http://cache.fleet.example.com:5000";
  publicKey = "cache.fleet.example.com:AAAA...=";  # contents of public-key.pem
};
```

This adds `cacheUrl` to `nix.settings.substituters` and the public key to `nix.settings.trusted-public-keys`.

## Agent fetch workflow

When a deploy is triggered, each agent resolves the closure from substituters in order:

1. `cacheUrl` from `services.nixfleet-cache` (or `services.nixfleet-agent.cacheUrl`)
2. Default Nix substituters (`cache.nixos.org`, etc.)

Agents automatically benefit from the fleet cache once the client module is enabled and the signing key is trusted - no additional configuration on the agent side is needed.

To override the cache URL per-deploy from the CLI:

```sh
nixfleet deploy --tags web --release REL-xxx --cache-url http://cache.fleet.example.com:5000
```

## Advanced: custom cache backends

For Attic, Cachix, or other cache backends that need a custom push command, use the `--push-hook` CLI flag:

```sh
# Attic example
nixfleet release create --push-to ssh://root@cache --push-hook "attic push fleet {}"

# Cachix example
nixfleet release create --push-hook "cachix push my-cache {}"
```

The `{}` placeholder is replaced with each store path. When combined with `--push-to`, the hook runs on the remote host via SSH. Without `--push-to`, it runs locally.

Fleet repos that want Attic can add it as their own flake input and configure it via plain NixOS modules.

### Attic and upstream dependencies

Attic is a **push-only** cache - it does not proxy upstream caches like `cache.nixos.org`. When you push a closure with `attic push`, Attic skips store paths that already exist in upstream caches to save bandwidth and storage. This means your private cache may not have every path needed to fetch a full closure.

The agent handles this automatically: if `nix copy --from <cache_url>` fails (e.g. a dependency like `kmod` exists on `cache.nixos.org` but not in your Attic cache), it falls back to `nix-store --realise` which uses the system-configured substituters. Your custom-built paths are still served from LAN (Attic), while standard nixpkgs dependencies fall through to `cache.nixos.org`.

**For air-gapped fleets** (no WAN access), you must push complete closures including all upstream dependencies. Use `nix copy --to` instead of `attic push` - it copies every path regardless of upstream availability:

```sh
# Push complete closure (all paths, no upstream skip)
nix copy --to http://cache:8081/fleet /nix/store/...-nixos-system-...

# Or via SSH to the cache host's store (harmonia serves it directly)
nix copy --to ssh://root@cache /nix/store/...-nixos-system-...
```

## See also

- [Cache Options](../../reference/cache-options.md) - full option reference
- [Secrets](../extending/secrets.md) - managing the signing key with agenix

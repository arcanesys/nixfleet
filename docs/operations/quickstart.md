# Quickstart

The minimum path from a fresh repo to a single managed host. For multi-host fleets, see [RFC-0001 (`fleet.nix`)](../rfcs/0001-fleet-nix.md) and the [operator cookbook](operator-cookbook.md). For the unattended signed-rollout machinery, see [architecture](../design/architecture.md) and [RFC-0002 (reconciler)](../rfcs/0002-reconciler.md).

## A minimal host

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixfleet.url = "github:arcanesys/nixfleet";
  };

  outputs = { nixpkgs, nixfleet, ... }: {
    nixosConfigurations.my-server = nixfleet.lib.mkHost {
      hostName = "my-server";
      platform = "x86_64-linux";
      modules = [
        nixfleet.scopes.persistence.impermanence
        nixfleet.scopes.secrets

        ./hardware-configuration.nix
        ({ ... }: {
          hostSpec.userName = "deploy";
          users.users.deploy = {
            isNormalUser = true;
            extraGroups = [ "wheel" ];
            openssh.authorizedKeys.keys = [ "ssh-ed25519 AAAA..." ];
          };
          services.nixfleet-agent = {
            enable = true;
            controlPlane.url = "https://cp.example.com:8080";
          };
        })
      ];
    };
  };
}
```

`mkHost` returns a standard `nixosSystem` (or `darwinSystem` for Darwin platforms). Nothing in the result is NixFleet-specific - if you remove the agent module, the host is a vanilla NixOS configuration deployable with stock tooling.

## Deploy

Standard NixOS / Darwin tooling, no NixFleet-specific glue:

```sh
nixos-anywhere --flake .#my-server root@192.168.1.50  # fresh install
sudo nixos-rebuild switch --flake .#my-server          # local rebuild
darwin-rebuild switch --flake .#my-mac                 # macOS
```

Fleet rollouts are git-driven from this point: commit -> CI signs -> CP polls `fleet.resolved.json` -> agents pull their per-host target on next checkin. There is no operator CLI verb between commit and host activation. See [operator cookbook -> Deploy a fleet change](operator-cookbook.md#deploy-a-fleet-change).

## Build and install the operator CLI

```sh
cargo build --release -p nixfleet-cli
install -m 0755 target/release/nixfleet ~/.local/bin/
```

Alternatively, run without installing: `nix run github:arcanesys/nixfleet#nixfleet-cli -- <subcommand>`.

## Initialise operator config

```sh
nixfleet config init \
  --cp-url https://cp.example.com:8080 \
  --ca-cert /etc/nixfleet/ca.pem \
  --client-cert ~/.config/nixfleet/operator.pem \
  --client-key  ~/.config/nixfleet/operator.key
```

Writes `~/.config/nixfleet/config.toml` (mode 0600). Override values per-invocation via flags or `NIXFLEET_*` environment variables. The flag > env > file precedence is locked in tests.

## Verify

```sh
nixfleet status                  # rendered fleet table
nixfleet status --json           # raw HostsResponse for piping
nixfleet rollout trace <id>      # wave-major dispatch history
```

For the full CLI surface (subcommands, flags, status-label precedence, pin markers), see [reference/crates/nixfleet-cli](../reference/crates/nixfleet-cli.md).

## Next steps

- Enrol additional hosts: [operator cookbook -> Add a host to the fleet](operator-cookbook.md#add-a-host-to-the-fleet)
- Mint a bootstrap token: [bootstrap-token-lifecycle](bootstrap-token-lifecycle.md)
- Test the loop locally on VMs first: [vm-lifecycle](vm-lifecycle.md)
- Verify your fleet config before pushing: [testing](testing.md)
- Recovery runbook if cp goes down: [disaster-recovery](disaster-recovery.md)

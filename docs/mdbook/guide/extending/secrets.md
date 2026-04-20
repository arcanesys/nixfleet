# Secrets

NixFleet provides a secrets wiring scope that handles identity path management, impermanence persistence, and boot ordering. Fleet repos bring their own backend (agenix, sops-nix) and wire it to the framework.

## Enabling the secrets scope

```nix
nixfleet.secrets.enable = true;
```

The scope computes `config.nixfleet.secrets.resolvedIdentityPaths` based on its options:
- **Servers** (`enableUserKey = false`, the default for the server role): host SSH key only (`/etc/ssh/ssh_host_ed25519_key`)
- **Workstations** (`enableUserKey = true`, the default for the workstation role): host SSH key + user key fallback (`~/.keys/id_ed25519`)

On impermanent hosts, identity keys are automatically persisted.

## agenix example

```nix
# flake.nix inputs
inputs.agenix.url = "github:ryantm/agenix";
inputs.agenix.inputs.nixpkgs.follows = "nixfleet/nixpkgs";

# In your host modules
{inputs, config, ...}: {
  imports = [inputs.agenix.nixosModules.default];

  # Use framework-computed identity paths
  age.identityPaths = config.nixfleet.secrets.resolvedIdentityPaths;

  age.secrets.root-password.file = "${inputs.secrets}/org/root-password.age";

  hostSpec = {
    hashedPasswordFile = config.age.secrets.root-password.path;
    rootHashedPasswordFile = config.age.secrets.root-password.path;
  };
}
```

## sops-nix example

```nix
# flake.nix inputs
inputs.sops-nix.url = "github:Mic92/sops-nix";
inputs.sops-nix.inputs.nixpkgs.follows = "nixfleet/nixpkgs";

# In your host modules
{inputs, config, ...}: {
  imports = [inputs.sops-nix.nixosModules.sops];

  sops = {
    defaultSopsFile = ./secrets/secrets.yaml;
    # sops-nix also uses age keys - resolvedIdentityPaths works here too
    age.keyFile = builtins.head config.nixfleet.secrets.resolvedIdentityPaths;
  };

  sops.secrets.root-password.neededForUsers = true;

  hostSpec = {
    hashedPasswordFile = config.sops.secrets.root-password.path;
    rootHashedPasswordFile = config.sops.secrets.root-password.path;
  };
}
```

## Extension points

hostSpec provides three options for wiring secrets into the framework:

| Option | Type | Purpose |
|--------|------|---------|
| `secretsPath` | `nullOr str` | Hint for the path to your secrets repo/directory. |
| `hashedPasswordFile` | `nullOr str` | Path to a hashed password file for the primary user. |
| `rootHashedPasswordFile` | `nullOr str` | Path to a hashed password file for root. |

When `hashedPasswordFile` or `rootHashedPasswordFile` is non-null, the core NixOS module sets `users.users.<name>.hashedPasswordFile` accordingly.

## Bootstrapping

New machines need a decryption key before they can decrypt secrets. Two approaches:

### --extra-files (nixos-anywhere)

Pass the key during initial install:

```sh
mkdir -p /tmp/extra/etc/ssh
cp /path/to/ssh_host_ed25519_key /tmp/extra/etc/ssh/ssh_host_ed25519_key
chmod 600 /tmp/extra/etc/ssh/ssh_host_ed25519_key

nixos-anywhere --flake .#myhost --extra-files /tmp/extra root@192.168.1.50
```

The `build-vm` and `test-vm` apps do this automatically when a key is found at `~/.keys/id_ed25519` or `~/.ssh/id_ed25519`. You can also pass a key explicitly with `--identity-key PATH`. For real hardware, pass `--extra-files` to `nixos-anywhere` to inject the key during install.

The secrets scope's `nixfleet-host-key-check` service auto-generates the host key at `/etc/ssh/ssh_host_ed25519_key` on first boot if the key is missing, so bootstrapping without a pre-provisioned key is safe.

### Generate on target

SSH into the machine and the host key will be generated automatically by `nixfleet-host-key-check` before sshd starts. Alternatively, generate one manually and add it to your secrets configuration:

```sh
ssh root@192.168.1.50
ssh-keygen -t ed25519 -f /etc/ssh/ssh_host_ed25519_key -N ""
```

Then extract the public key, add it to your secrets configuration (e.g., `secrets.nix` for agenix), and re-encrypt the affected secrets.

## Key placement on impermanent hosts

On impermanent hosts, the secrets scope automatically persists:
- `/etc/ssh/ssh_host_ed25519_key` (and `.pub`)
- The user key directory (`~/.keys`) when `enableUserKey` is true

The impermanence scope also persists `~/.keys` independently, providing defense in depth.

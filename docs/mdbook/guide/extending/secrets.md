# Secrets

NixFleet is secrets-agnostic. It provides extension points for secrets integration but does not prescribe a specific tool. The two most common choices are [agenix](https://github.com/ryantm/agenix) and [sops-nix](https://github.com/Mic92/sops-nix); both work as fleet-level modules.

## Extension points

hostSpec provides three options for wiring secrets into the framework:

| Option | Type | Purpose |
|--------|------|---------|
| `secretsPath` | `nullOr str` | Hint for the path to your secrets repo/directory. Framework-agnostic. |
| `hashedPasswordFile` | `nullOr str` | Path to a hashed password file for the primary user. |
| `rootHashedPasswordFile` | `nullOr str` | Path to a hashed password file for root. |

When `hashedPasswordFile` or `rootHashedPasswordFile` is non-null, the core NixOS module sets `users.users.<name>.hashedPasswordFile` accordingly.

## agenix example

```nix
# flake.nix inputs
inputs.agenix.url = "github:ryantm/agenix";
inputs.agenix.inputs.nixpkgs.follows = "nixfleet/nixpkgs";

# In your host modules
{inputs, config, ...}: let
  secretsPath = "/path/to/secrets";
in {
  imports = [inputs.agenix.nixosModules.default];

  age.identityPaths = ["/persist/home/${config.hostSpec.userName}/.keys/id_ed25519"];

  age.secrets.user-password = {
    file = "${secretsPath}/user-password.age";
    owner = config.hostSpec.userName;
  };
  age.secrets.root-password.file = "${secretsPath}/root-password.age";

  hostSpec = {
    secretsPath = secretsPath;
    hashedPasswordFile = config.age.secrets.user-password.path;
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
    age.keyFile = "/persist/home/${config.hostSpec.userName}/.keys/age-key.txt";
  };

  sops.secrets.user-password.neededForUsers = true;
  sops.secrets.root-password.neededForUsers = true;

  hostSpec = {
    hashedPasswordFile = config.sops.secrets.user-password.path;
    rootHashedPasswordFile = config.sops.secrets.root-password.path;
  };
}
```

## Bootstrapping

New machines need a decryption key before they can decrypt secrets. Two approaches:

### --extra-files (nixos-anywhere)

Pass the key during initial install:

```sh
mkdir -p /tmp/extra/persist/home/alice/.keys
cp ~/.keys/id_ed25519 /tmp/extra/persist/home/alice/.keys/id_ed25519
chmod 600 /tmp/extra/persist/home/alice/.keys/id_ed25519

nixos-anywhere --flake .#myhost --extra-files /tmp/extra root@192.168.1.50
```

The `spawn-qemu` and `test-vm` apps do this automatically when a key is found at `~/.keys/id_ed25519` or `~/.ssh/id_ed25519`.

### Generate on target

SSH into the machine and generate a new age key:

```sh
ssh root@192.168.1.50
mkdir -p /persist/home/alice/.keys
age-keygen -o /persist/home/alice/.keys/age-key.txt
```

Then add the public key to your secrets configuration and re-encrypt.

## Key placement on impermanent hosts

On impermanent hosts, keys must live under `/persist`. The framework's impermanence scope automatically persists `~/.keys`. The activation script ensures correct ownership of `/persist/home/<userName>` and the `.keys` directory within it.

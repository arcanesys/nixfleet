# Design Guarantees

These are not features you enable. They are properties that emerge from the architecture.

| Property | What it means | How the architecture delivers it |
|---|---|---|
| **Reproducibility** | Same configuration produces an identical system, every time, on any machine. | The Nix store is content-addressed - every package is identified by a cryptographic hash of its inputs. `flake.lock` pins every dependency to an exact revision. The `follows` chain ensures nixpkgs, home-manager, disko, and impermanence all resolve to one consistent version. |
| **Immutability** | Running systems cannot drift from their declared configuration. | The Nix store is read-only - no process can modify installed software in place. With optional ephemeral root (impermanence), the entire root filesystem is wiped and recreated from configuration on every boot, eliminating accumulated state. |
| **Atomic rollback** | Recover from any deployment in seconds, not minutes. | NixOS generations are atomic filesystem switches - the previous generation remains intact in the Nix store. The fleet agent auto-rolls back on health check failure. Manual rollback is a single command: `nixfleet rollback --host web-01 --ssh`. |
| **Auditability** | Every change to every system is traceable to a commit. | Configuration is Git-native - the entire system state is defined in version-controlled Nix files. The control plane maintains a deployment audit log, a release registry (immutable manifests of per-host store paths), and a rollout event timeline for every host. Releases can be diffed with `nixfleet release diff <A> <B>`. |
| **Supply chain integrity** | The complete dependency tree of every system is known and verifiable. | `flake.lock` records the cryptographic hash of every input. Builds are reproducible - the same inputs always produce the same output hash. No implicit dependencies, no untracked downloads during build. |
| **Graceful degradation** | The fleet survives a control plane outage without disruption. | The architecture uses a polling model - agents independently pull desired state on a configurable interval (default: 60s, with a `poll_hint`-driven fast path of 5s during active rollouts, and 30s retries on transient failures). If the control plane is unreachable, agents continue running their last-known-good generation. There is no single point of failure; each host is a self-contained NixOS system that operates independently. |

These properties hold whether you use the full orchestration layer or just `mkHost` with standard NixOS commands.

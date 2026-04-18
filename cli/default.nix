{
  lib,
  rustPlatform,
  git,
}:
# See ../agent/default.nix — same thin alias to the shared workspace
# derivation. `$out/bin/nixfleet` (the CLI's actual binary name) is
# present in the workspace output.
import ../cargo-workspace.nix {inherit lib rustPlatform git;}

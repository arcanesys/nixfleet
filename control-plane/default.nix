{
  lib,
  rustPlatform,
}:
# See ../agent/default.nix — same thin alias to the shared workspace
# derivation. `$out/bin/nixfleet-control-plane` is present in the
# workspace output so every caller still gets the binary it expected.
import ../cargo-workspace.nix {inherit lib rustPlatform;}

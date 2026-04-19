{
  lib,
  rustPlatform,
}:
# Thin alias to the unified workspace derivation. Every callsite of
# `pkgs.callPackage ../agent {}` (scopes/_agent.nix systemd package,
# VM tests, etc.) receives the same workspace drv that
# `.#packages.<system>.nixfleet-agent` returns — so all consumers share
# one build, not one each. The workspace output contains
# `$out/bin/nixfleet-agent`, which is what every caller expects.
#
# See `cargo-workspace.nix` at the repo root for the long-form rationale.
import ../cargo-workspace.nix {inherit lib rustPlatform;}

{
  lib,
  rustPlatform,
}:
# Single workspace build that produces every NixFleet binary in one
# `cargo build` + `cargo test` pass. Replaces three previously-separate
# `rustPlatform.buildRustPackage` derivations (agent, control-plane,
# cli) that each re-ran `cargo test --workspace` from scratch in the
# nix sandbox — triple work for identical source trees.
#
# `packages.nixfleet-{agent,control-plane,cli}` in `agent-package.nix`
# all point at this same derivation, so `nix build .#packages.*` on
# any of the three is a cache hit after the first build.
#
# The output contains three binaries:
#   $out/bin/nixfleet-agent
#   $out/bin/nixfleet-control-plane
#   $out/bin/nixfleet            (the CLI binary name, not nixfleet-cli)
rustPlatform.buildRustPackage {
  pname = "nixfleet-workspace";
  version = "0.1.0";
  # Same scoped fileset every per-crate derivation used. Editing files
  # outside the Rust workspace (docs, Nix modules, TODO.md) does not
  # invalidate the build hash.
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./agent
      ./control-plane
      ./cli
      ./shared
    ];
  };
  cargoLock.lockFile = ./Cargo.lock;

  # No `cargoBuildFlags` — build the whole workspace in one invocation.
  # No `cargoTestFlags` — `cargo test` at the workspace root runs every
  # crate's tests, which is what the validate script used to get by
  # calling three separate package builds sequentially.

  meta = {
    description = "NixFleet workspace (agent + control-plane + CLI)";
    license = lib.licenses.asl20;
    # The convention for `mainProgram` is the primary CLI. Any of the
    # three binaries is a valid entry point, but the operator-facing
    # CLI is the one users typically run.
    mainProgram = "nixfleet";
  };
}

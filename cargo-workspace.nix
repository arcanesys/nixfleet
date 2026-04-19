{
  lib,
  rustPlatform,
}:
# Single workspace build that produces every NixFleet binary in one
# `cargo build` + `cargo test` pass, sharing sandbox test work across
# the agent, control plane, and CLI binaries.
#
# `packages.nixfleet-{agent,control-plane,cli}` in `agent-package.nix`
# all alias this derivation, so `nix build .#packages.*` on any of the
# three produces a single workspace build plus cheap symlink wrappers.
#
# Output binaries:
#   $out/bin/nixfleet-agent
#   $out/bin/nixfleet-control-plane
#   $out/bin/nixfleet            (the CLI binary name, not nixfleet-cli)
rustPlatform.buildRustPackage {
  pname = "nixfleet-workspace";
  version = "0.1.0";
  # Scoped fileset: editing files outside the Rust workspace (docs,
  # Nix modules, TODO.md) does not invalidate the build hash.
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
  # crate's tests, which is the point of unifying the build.

  meta = {
    description = "NixFleet workspace (agent + control-plane + CLI)";
    license = lib.licenses.asl20;
    # `mainProgram` is the operator CLI. Each alias in agent-package.nix
    # overrides this per-binary via its own wrapper derivation.
    mainProgram = "nixfleet";
  };
}

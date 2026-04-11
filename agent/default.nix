{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "nixfleet-agent";
  version = "0.1.0";
  # Scope `src` to just the Rust workspace files. Using `./..` (the
  # repo root) pulls every file in the repo — test fixtures, docs,
  # TODO.md, module configs — into the build hash, so editing any of
  # them invalidates the Rust cache and forces a full rebuild. The
  # fileset below pins src to the cargo workspace metadata (top-level
  # Cargo.toml + Cargo.lock) plus every crate directory the workspace
  # references. Editing anything outside these no longer triggers a
  # Rust rebuild.
  src = lib.fileset.toSource {
    root = ./..;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../agent
      ../control-plane
      ../cli
      ../shared
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = ["-p" "nixfleet-agent"];

  meta = {
    description = "NixFleet fleet management agent";
    license = lib.licenses.asl20;
    mainProgram = "nixfleet-agent";
  };
}

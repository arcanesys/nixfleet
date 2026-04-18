{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "nixfleet-cli";
  version = "0.1.0";
  # See agent/default.nix for the rationale behind the fileset scope.
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
  cargoBuildFlags = ["-p" "nixfleet-cli"];

  meta = {
    description = "NixFleet fleet management CLI";
    license = lib.licenses.asl20;
    mainProgram = "nixfleet";
  };
}

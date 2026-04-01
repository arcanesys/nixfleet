{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "nixfleet-cli";
  version = "0.1.0";
  src = ./..;
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = ["-p" "nixfleet-cli"];

  meta = {
    description = "NixFleet fleet management CLI";
    license = lib.licenses.asl20;
    mainProgram = "nixfleet";
  };
}

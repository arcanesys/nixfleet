{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "nixfleet-agent";
  version = "0.1.0";
  src = ./..;
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = ["-p" "nixfleet-agent"];

  meta = {
    description = "NixFleet fleet management agent";
    license = lib.licenses.asl20;
    mainProgram = "nixfleet-agent";
  };
}

{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "nixfleet-control-plane";
  version = "0.1.0";
  src = ./..;
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = ["-p" "nixfleet-control-plane"];

  meta = {
    description = "NixFleet control plane server";
    license = lib.licenses.asl20;
    mainProgram = "nixfleet-control-plane";
  };
}

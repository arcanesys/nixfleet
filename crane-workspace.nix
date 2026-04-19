# Crane-based workspace build — layered caching for independent packages,
# rebuild isolation, and shared dependency artifacts.
#
# Layers:
#   1. cargoArtifacts (buildDepsOnly) — shared compiled deps, only rebuilds
#      when Cargo.toml/Cargo.lock change
#   2. Per-crate packages (buildPackage) — scoped source per crate, doCheck=false
#   3. workspace-tests (cargoTest) — one test run for the whole workspace
#
# Rebuild isolation works because Cargo.toml uses `members = ["crates/*"]`
# (glob). When a crate's directory is absent from the source, cargo just
# doesn't find it via the glob — no error. Each per-crate build only
# includes its own source + shared, so changing agent/src doesn't
# invalidate cli's derivation hash.
{
  lib,
  craneLib,
}: let
  # Full workspace source — used for deps and tests.
  workspaceSrc = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  # Layer 1: compiled dependencies — shared across all crate builds.
  # Rebuilds only when Cargo.toml / Cargo.lock change.
  cargoArtifacts = craneLib.buildDepsOnly {
    src = workspaceSrc;
    pname = "nixfleet-workspace-deps";
  };

  # Helper: per-crate fileset — workspace root manifests + target crate + shared.
  # Other crates' dirs are excluded; the glob in Cargo.toml tolerates their absence.
  # `extraFiles` allows including non-Rust files (e.g. SQL migrations).
  fileSetForCrate = {
    crate,
    extraFiles ? [],
  }:
    lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions ([
          ./Cargo.toml
          ./Cargo.lock
          (craneLib.fileset.commonCargoSources ./crates/shared)
          (craneLib.fileset.commonCargoSources crate)
        ]
        ++ extraFiles);
    };

  commonArgs = {
    inherit cargoArtifacts;
    version = "0.1.0";
    doCheck = false;
  };

  # Layer 2: per-crate packages — independent derivations with scoped source.
  nixfleet-agent = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-agent";
      cargoExtraArgs = "-p nixfleet-agent";
      src = fileSetForCrate {crate = ./crates/agent;};
      meta = {
        description = "NixFleet fleet management agent";
        license = lib.licenses.asl20;
        mainProgram = "nixfleet-agent";
      };
    });

  nixfleet-control-plane = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-control-plane";
      cargoExtraArgs = "-p nixfleet-control-plane";
      src = fileSetForCrate {
        crate = ./crates/control-plane;
        extraFiles = [./crates/control-plane/migrations];
      };
      meta = {
        description = "NixFleet control plane server";
        license = lib.licenses.asl20;
        mainProgram = "nixfleet-control-plane";
      };
    });

  nixfleet-cli = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-cli";
      cargoExtraArgs = "-p nixfleet-cli";
      src = fileSetForCrate {crate = ./crates/cli;};
      meta = {
        description = "NixFleet fleet management CLI";
        license = lib.licenses.asl20;
        mainProgram = "nixfleet";
      };
    });

  # Layer 3: workspace tests — one run covering all crates.
  workspace-tests = craneLib.cargoTest {
    inherit cargoArtifacts;
    src = workspaceSrc;
    pname = "nixfleet-workspace-tests";
    version = "0.1.0";
    cargoExtraArgs = "--workspace --locked";
  };
in {
  packages = {inherit nixfleet-agent nixfleet-control-plane nixfleet-cli;};
  checks = {inherit workspace-tests;};
}

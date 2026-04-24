# Crane-based workspace build — layered caching for independent packages,
# rebuild isolation, and shared dependency artifacts.
#
# Layers:
#   1. cargoArtifacts (buildDepsOnly) — shared compiled deps
#   2. Per-crate packages (buildPackage) — scoped source per crate, doCheck=false
#   3. workspace-tests (cargoTest) — one test run for the whole workspace
{
  lib,
  craneLib,
}: let
  workspaceSrc = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  cargoArtifacts = craneLib.buildDepsOnly {
    src = workspaceSrc;
    pname = "nixfleet-workspace-deps";
  };

  # Per-crate fileset. Shares the three v0.2 library crates
  # (nixfleet-proto, nixfleet-canonicalize, nixfleet-reconciler) so
  # every binary crate has access to the common boundary-contract +
  # canonicalization surface. `extraFiles` lets callers include
  # non-Rust files (e.g. SQL migrations under crates/*/migrations/).
  fileSetForCrate = {
    crate,
    extraFiles ? [],
  }:
    lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions ([
          ./Cargo.toml
          ./Cargo.lock
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-proto)
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-canonicalize)
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-reconciler)
          (craneLib.fileset.commonCargoSources crate)
        ]
        ++ extraFiles);
    };

  commonArgs = {
    inherit cargoArtifacts;
    version = "0.2.0";
    doCheck = false;
  };

  nixfleet-agent = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-agent";
      cargoExtraArgs = "-p nixfleet-agent";
      src = fileSetForCrate {crate = ./crates/nixfleet-agent;};
      meta = {
        description = "NixFleet fleet management agent (v0.2 poll-only skeleton)";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-agent";
      };
    });

  nixfleet-control-plane = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-control-plane";
      cargoExtraArgs = "-p nixfleet-control-plane";
      src = fileSetForCrate {
        crate = ./crates/nixfleet-control-plane;
        extraFiles = [./crates/nixfleet-control-plane/migrations];
      };
      meta = {
        description = "NixFleet v0.2 control plane skeleton";
        license = lib.licenses.agpl3Only;
        mainProgram = "nixfleet-control-plane";
      };
    });

  nixfleet-cli = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-cli";
      cargoExtraArgs = "-p nixfleet-cli";
      src = fileSetForCrate {crate = ./crates/nixfleet-cli;};
      meta = {
        description = "NixFleet v0.2 operator CLI";
        license = lib.licenses.mit;
        mainProgram = "nixfleet";
      };
    });

  nixfleet-canonicalize = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-canonicalize";
      cargoExtraArgs = "-p nixfleet-canonicalize";
      src = fileSetForCrate {crate = ./crates/nixfleet-canonicalize;};
      meta = {
        description = "JCS (RFC 8785) canonicalizer pinned per CONTRACTS.md §III";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-canonicalize";
      };
    });

  nixfleet-verify-artifact = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-verify-artifact";
      cargoExtraArgs = "-p nixfleet-verify-artifact";
      src = fileSetForCrate {crate = ./crates/nixfleet-verify-artifact;};
      meta = {
        description = "Phase 2 harness CLI wrapping nixfleet_reconciler::verify_artifact";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-verify-artifact";
      };
    });

  workspace-tests = craneLib.cargoTest {
    inherit cargoArtifacts;
    src = workspaceSrc;
    pname = "nixfleet-workspace-tests";
    version = "0.2.0";
    cargoExtraArgs = "--workspace --locked";
  };
in {
  packages = {inherit nixfleet-agent nixfleet-control-plane nixfleet-cli nixfleet-canonicalize nixfleet-verify-artifact;};
  checks = {inherit workspace-tests;};
}

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

  # Per-crate source: always includes the three shared library crates; `extraFiles` for non-Rust files (e.g. SQL migrations).
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

  # CLI's e2e test depends on nixfleet-control-plane via [dev-dependencies]
  # path = "../nixfleet-control-plane". Cargo refuses to load the CLI manifest
  # without CP's manifest also being readable, even when only -p nixfleet-cli
  # is built. Pull CP's sources + migrations into the CLI's per-crate source
  # filter so the manifest resolves; doCheck = false means the e2e test
  # itself never runs under nix.
  nixfleet-cli = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-cli";
      cargoExtraArgs = "-p nixfleet-cli";
      src = fileSetForCrate {
        crate = ./crates/nixfleet-cli;
        extraFiles = [
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-control-plane)
          ./crates/nixfleet-control-plane/migrations
        ];
      };
      meta = {
        description = "NixFleet operator CLI: nixfleet (status, rollout trace, config init, derive-pubkey, mint-operator-cert, mint-token)";
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
        description = "JCS (RFC 8785) canonicalizer pinned per docs/design/contracts.md §III";
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

  nixfleet-release = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-release";
      cargoExtraArgs = "-p nixfleet-release";
      src = fileSetForCrate {crate = ./crates/nixfleet-release;};
      meta = {
        description = "Producer for fleet.resolved.json (CONTRACTS §I #1) - orchestrates build/inject/canonicalize/sign/release";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-release";
      };
    });

  workspace-tests = craneLib.cargoTest {
    inherit cargoArtifacts;
    src = workspaceSrc;
    pname = "nixfleet-workspace-tests";
    version = "0.2.0";
    cargoExtraArgs = "--workspace --locked";
  };

  cargoDocs = craneLib.cargoDoc (commonArgs
    // {
      src = workspaceSrc;
      pname = "nixfleet-cargo-doc";
      cargoDocExtraArgs = "--workspace --document-private-items --no-deps";
    });
in {
  packages = {
    inherit
      nixfleet-agent
      nixfleet-control-plane
      nixfleet-cli
      nixfleet-canonicalize
      nixfleet-verify-artifact
      nixfleet-release
      ;
  };
  checks = {inherit workspace-tests;};
  inherit cargoDocs;
}

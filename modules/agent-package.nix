{...}: {
  # Every NixFleet Rust binary (agent, control-plane, CLI) comes out of
  # one shared `cargo-workspace.nix` derivation, so `nix build` of any
  # `.#packages.*.nixfleet-*` triggers ONE workspace build; subsequent
  # calls are cache hits. Historically each crate was its own
  # `rustPlatform.buildRustPackage` and each re-ran `cargo test` over
  # the whole workspace in the sandbox — triple work for identical
  # source trees. See `cargo-workspace.nix` for the long-form rationale.
  #
  # Each `packages.nixfleet-*` attr is a tiny symlink wrapper around the
  # workspace rather than the workspace itself, so:
  #   1. `mainProgram` is per-binary — `nix run .#nixfleet-agent` runs
  #      `bin/nixfleet-agent`, not the CLI.
  #   2. Inspecting `.#packages.*.nixfleet-agent` via `nix path-info`
  #      still shows a derivation whose `pname` matches the attr name.
  #   3. The wrapper has the workspace as a runtime dep, so building
  #      any of the three aliases builds the workspace exactly once.
  perSystem = {pkgs, ...}: let
    workspace = pkgs.callPackage ../cargo-workspace.nix {};

    # Produce a derivation whose `$out/bin/<binary>` is a symlink into
    # the workspace output. `meta.mainProgram` is set so `nix run`
    # picks the right binary without falling back to the first one in
    # bin/ alphabetical order.
    mkAlias = {
      pname,
      binary,
      description,
    }:
      pkgs.runCommand pname {
        inherit pname;
        inherit (workspace) version;
        meta = {
          inherit description;
          mainProgram = binary;
          inherit (workspace.meta) license;
        };
        passthru.workspace = workspace;
      } ''
        mkdir -p $out/bin
        ln -s ${workspace}/bin/${binary} $out/bin/${binary}
      '';
  in {
    # `nixfleet-workspace` is the raw workspace build. Exposed mostly
    # for the validate script's pre-build step so `nix build` has a
    # single target to warm the cache before the per-alias reports.
    packages.nixfleet-workspace = workspace;
    packages.nixfleet-agent = mkAlias {
      pname = "nixfleet-agent";
      binary = "nixfleet-agent";
      description = "NixFleet fleet management agent";
    };
    packages.nixfleet-control-plane = mkAlias {
      pname = "nixfleet-control-plane";
      binary = "nixfleet-control-plane";
      description = "NixFleet control plane server";
    };
    packages.nixfleet-cli = mkAlias {
      pname = "nixfleet-cli";
      binary = "nixfleet";
      description = "NixFleet fleet management CLI";
    };

    apps.agent = {
      type = "app";
      program = "${workspace}/bin/nixfleet-agent";
      meta.description = "NixFleet fleet management agent";
    };

    apps.control-plane = {
      type = "app";
      program = "${workspace}/bin/nixfleet-control-plane";
      meta.description = "NixFleet control plane server";
    };

    apps.nixfleet = {
      type = "app";
      program = "${workspace}/bin/nixfleet";
      meta.description = "NixFleet fleet management CLI";
    };
  };
}

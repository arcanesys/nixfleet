# Per-crate crane build — called via pkgs.callPackage from service modules.
# `inputs` is available via specialArgs (injected by mkHost).
{
  lib,
  pkgs,
  inputs,
}:
(import ../../crane-workspace.nix {
  inherit lib pkgs;
  craneLib = inputs.crane.mkLib pkgs;
})
.packages
.nixfleet-agent

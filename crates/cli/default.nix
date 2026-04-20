# Per-crate crane build - called via pkgs.callPackage from service modules.
{
  lib,
  pkgs,
  inputs,
}:
(import ../../crane-workspace.nix {
  inherit lib;
  craneLib = inputs.crane.mkLib pkgs;
})
.packages
.nixfleet-cli

# FOOTGUN: Darwin returns empty; aarch64-darwin pkgs.OVMF is broken upstream.
{inputs}: {pkgs}: let
  platform = import ./vm-platform.nix {inherit inputs;} {inherit pkgs;};
  scripts = {
    "build-vm" = import ./vm-scripts/build.nix {inherit platform pkgs;};
    "start-vm" = import ./vm-scripts/start.nix {inherit platform pkgs;};
    "stop-vm" = import ./vm-scripts/stop.nix {inherit platform pkgs;};
    "clean-vm" = import ./vm-scripts/clean.nix {inherit platform pkgs;};
    "test-vm" = import ./vm-scripts/test.nix {inherit platform pkgs;};
  };
in
  pkgs.lib.optionalAttrs platform.isLinux scripts

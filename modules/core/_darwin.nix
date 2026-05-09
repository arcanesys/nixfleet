{
  config,
  lib,
  ...
}: let
  hS = config.hostSpec;
in {
  imports = [../../contracts/trust.nix];

  system.stateVersion = lib.mkDefault 4;
  # GOTCHA: Darwin flake setups don't set NIX_PATH; verifyNixPath would fail.
  system.checks.verifyNixPath = false;
  system.primaryUser = "${hS.userName}";

  hostSpec.isDarwin = true;
}

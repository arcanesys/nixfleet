# LOADBEARING: not exported as a flakeModule; consumer fleets bring their own formatter.
{inputs, ...}: {
  imports = [inputs.treefmt-nix.flakeModule];

  perSystem = {...}: {
    treefmt = {
      projectRootFile = "flake.nix";
      programs = {
        alejandra.enable = true;
        shfmt.enable = true;
        deadnix.enable = true;
      };
    };
  };
}

# treefmt-nix: format and lint all languages with one command.
# `nix fmt` formats nix (alejandra), shell (shfmt), and checks for dead code (deadnix).
{inputs, ...}: {
  imports = [inputs.treefmt-nix.flakeModule];

  perSystem = {...}: {
    treefmt = {
      projectRootFile = "flake.nix";
      programs = {
        alejandra.enable = true; # Nix formatter
        shfmt.enable = true; # Shell formatter
        deadnix.enable = true; # Dead code detection
      };
    };
  };
}

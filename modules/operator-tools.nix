# Operator-side shell tools — built once via nix, run on operator
# workstations. Distinct from `apps.nix` (single-flake-host scripts
# like `validate`) and `rust-packages.nix` (workspace crates).
{...}: {
  perSystem = {pkgs, ...}: let
    nixfleet-trust-bootstrap = import ../tools/trust-bootstrap {inherit pkgs;};
  in {
    packages.nixfleet-trust-bootstrap = nixfleet-trust-bootstrap;

    apps.nixfleet-trust-bootstrap = {
      type = "app";
      program = "${nixfleet-trust-bootstrap}/bin/nixfleet-trust-bootstrap";
      meta.description = "Mint offline fleet root CA + sign TPM-bound issuance CA cert";
    };
  };
}

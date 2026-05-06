# Operator-side shell tools — built once via nix, run on operator
# workstations. Distinct from `apps.nix` (single-flake-host scripts
# like `validate`) and `rust-packages.nix` (workspace crates).
{...}: {
  perSystem = {pkgs, ...}: let
    nixfleet-cp-bootstrap = import ../tools/cp-bootstrap {inherit pkgs;};
  in {
    packages.nixfleet-cp-bootstrap = nixfleet-cp-bootstrap;

    apps.nixfleet-cp-bootstrap = {
      type = "app";
      program = "${nixfleet-cp-bootstrap}/bin/nixfleet-cp-bootstrap";
      meta.description = "Bundle C operator bootstrap — fleet root CA + TPM-bound issuance CA cert (nixfleet#41)";
    };
  };
}

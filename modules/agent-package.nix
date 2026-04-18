{...}: {
  perSystem = {pkgs, ...}: {
    packages.nixfleet-agent = pkgs.callPackage ../agent {};
    packages.nixfleet-control-plane = pkgs.callPackage ../control-plane {};
    packages.nixfleet-cli = pkgs.callPackage ../cli {};

    apps.agent = {
      type = "app";
      program = "${pkgs.callPackage ../agent {}}/bin/nixfleet-agent";
      meta.description = "NixFleet fleet management agent";
    };

    apps.control-plane = {
      type = "app";
      program = "${pkgs.callPackage ../control-plane {}}/bin/nixfleet-control-plane";
      meta.description = "NixFleet control plane server";
    };

    apps.nixfleet = {
      type = "app";
      program = "${pkgs.callPackage ../cli {}}/bin/nixfleet";
      meta.description = "NixFleet fleet management CLI";
    };
  };
}

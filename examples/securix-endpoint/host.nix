# Host-specific configuration for lab-endpoint.
{lib, ...}: {
  # Sécurix manages users itself via `securix.users` + `securix.self.user`.
  # Operator SSH access goes through root (nixos-anywhere bootstrap) plus
  # Sécurix's own user model after boot.
  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 NixfleetExampleKeyReplaceWithYourOwn"
  ];

  # Sécurix identity metadata.
  securix.self = {
    mainDisk = "/dev/vda";
    edition = "pilot";
    user = {
      email = "operator@example.gouv.fr";
      username = "operator";
    };
    machine = {
      serialNumber = "PILOT0001";
      inventoryId = 1;
      hardwareSKU = "t14g6";
      users = [];
    };
  };

  # Optional desktop. Variants: "kde" (default), "gnome", ...
  securix.graphical-interface.enable = true;
  securix.graphical-interface.variant = lib.mkDefault "kde";

  # To enroll under a NixFleet control plane, uncomment:
  # services.nixfleet-agent = {
  #   enable = true;
  #   controlPlaneUrl = "https://cp.example.internal:8443";
  #   tags = ["endpoint" "securix"];
  # };

  system.stateVersion = "24.11";
}

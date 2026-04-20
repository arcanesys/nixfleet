# Host-specific configuration for the lab-endpoint.
{lib, ...}: {
  # Operators - declarative user inventory
  nixfleet.operators = {
    primaryUser = "operator";
    rootSshKeys = [
      "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
    ];
    users.operator = {
      isAdmin = true;
      homeManager.enable = false;
      sshAuthorizedKeys = [
        "ssh-ed25519 NixfleetDemoKeyReplaceWithYourOwn"
      ];
    };
  };

  # Sécurix identity metadata
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

  # Graphical desktop
  securix.graphical-interface.enable = true;
  securix.graphical-interface.variant = lib.mkDefault "kde";

  system.stateVersion = "24.11";
}

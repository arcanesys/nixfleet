# Example: Single machine in its own repo.
#
# No flake-parts, no fleet modules, no import-tree.
# Just nixfleet + one host.
#
# Deploy:
#   nixos-anywhere --flake .#myhost root@192.168.1.50
#   sudo nixos-rebuild switch --flake .#myhost
{
  inputs = {
    nixfleet.url = "github:arcanesys/nixfleet";
    nixpkgs.follows = "nixfleet/nixpkgs";
  };

  outputs = {nixfleet, ...}: {
    nixosConfigurations.myhost = nixfleet.lib.mkHost {
      hostName = "myhost";
      platform = "x86_64-linux";
      hostSpec = {
        userName = "alice";
        timeZone = "US/Eastern";
        locale = "en_US.UTF-8";
        sshAuthorizedKeys = [
          "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA..."
        ];
      };
      modules = [
        ./hardware-configuration.nix
        ./disk-config.nix
      ];
    };
  };
}

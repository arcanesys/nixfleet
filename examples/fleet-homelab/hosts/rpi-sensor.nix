{...}: {
  networking.hostName = "rpi-sensor-01";
  nixpkgs.hostPlatform = "aarch64-linux";
  system.stateVersion = "25.11";
  boot.loader.grub.device = "nodev";
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };
}

{...}: {
  networking.hostName = "m70q-attic";
  system.stateVersion = "25.11";
  boot.loader.grub.device = "nodev";
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };
}

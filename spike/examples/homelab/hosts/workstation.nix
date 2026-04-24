{...}: {
  system.stateVersion = "25.11";
  boot.loader.grub.device = "nodev";
  fileSystems."/" = {
    device = "/dev/null";
    fsType = "ext4";
  };
}

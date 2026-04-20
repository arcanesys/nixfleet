# Placeholder — replace with output of `nixos-generate-config --show-hardware-config`
{
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };
  boot.loader.systemd-boot.enable = true;
}

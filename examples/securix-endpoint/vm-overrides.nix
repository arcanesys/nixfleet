# VM overrides - disable Secure Boot and LUKS (no TPM/passphrase in QEMU).
# Import this module for VM testing; omit for real hardware deploys.
{lib, ...}: {
  # Disable Secure Boot (no TPM in QEMU)
  boot.lanzaboote.enable = false;
  boot.loader.systemd-boot.enable = true;

  # Virtio drivers for initrd disk detection
  boot.initrd.availableKernelModules = ["virtio_pci" "virtio_blk" "virtio_scsi"];

  # Flat disk layout (no LUKS - avoids passphrase prompt in VM)
  securix.filesystems.enable = false;
  disko.devices.disk.main = {
    device = "/dev/vda";
    type = "disk";
    content = {
      type = "gpt";
      partitions = {
        ESP = {
          end = "512M";
          type = "EF00";
          content = {
            type = "filesystem";
            format = "vfat";
            mountpoint = "/boot";
            mountOptions = ["umask=0077"];
          };
        };
        root = {
          size = "100%";
          content = {
            type = "filesystem";
            format = "ext4";
            mountpoint = "/";
            extraArgs = ["-L" "nixos"];
          };
        };
      };
    };
  };

  # Password for graphical login (SDDM) - "changeme"
  users.users.operator.hashedPassword = lib.mkForce "$6$gkBTmLDGP5NIkZpw$wgSG8D29EA1MfR6S27ypVq2ahAN9js3Fvsz.8auDlDlzR/P2mgsABIAicWMKf9JcT1p9VISXPkrfdvNg/VHDp1";
}

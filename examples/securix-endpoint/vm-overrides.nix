# VM-mode overrides - disable Secure Boot + LUKS (no TPM / passphrase
# in QEMU), provide a flat disk layout, set a known root password.
# Import for VM testing only; omit for real-hardware deploys.
{lib, ...}: {
  # No TPM in QEMU - fall back to systemd-boot.
  boot.lanzaboote.enable = lib.mkForce false;
  boot.loader.systemd-boot.enable = true;

  # Virtio drivers for initrd disk detection.
  boot.initrd.availableKernelModules = ["virtio_pci" "virtio_blk" "virtio_scsi"];

  # Flat disk - skips Sécurix's LUKS+impermanence layout (which prompts
  # for a passphrase that can't be entered through QEMU's console).
  securix.filesystems.enable = lib.mkForce false;
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

  # Password for SDDM login - "changeme". yescrypt hash:
  #   mkpasswd -m yescrypt changeme
  users.users.operator.hashedPassword = lib.mkForce "$6$gkBTmLDGP5NIkZpw$wgSG8D29EA1MfR6S27ypVq2ahAN9js3Fvsz.8auDlDlzR/P2mgsABIAicWMKf9JcT1p9VISXPkrfdvNg/VHDp1";
}

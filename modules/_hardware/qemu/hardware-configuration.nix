{
  lib,
  modulesPath,
  ...
}: {
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  boot.initrd.availableKernelModules = ["ahci" "xhci_pci" "virtio_pci" "virtio_blk" "virtio_scsi" "sr_mod"];
  boot.initrd.kernelModules = [];
  boot.kernelModules = ["kvm-intel" "kvm-amd"];
  boot.extraModulePackages = [];

  # Bootloader defaults for test VMs. mk-host no longer injects
  # bootloader config (that's host-hardware-specific); the qemu profile
  # owns it for the isVm case.
  boot.loader.systemd-boot.enable = lib.mkDefault true;
  boot.loader.efi.canTouchEfiVariables = lib.mkDefault true;
}

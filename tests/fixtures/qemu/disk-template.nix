# @root is wiped at boot by the impermanence scope.
{
  lib,
  disk ? "/dev/vda",
  espSize ? "512M",
  withSwap ? false,
  swapSize ? "8",
  ...
}: {
  disk = {
    disk0 = {
      type = "disk";
      device = disk;
      content = {
        type = "gpt";
        partitions = {
          ESP = {
            priority = 1;
            name = "ESP";
            start = "1M";
            end = espSize;
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
              type = "btrfs";
              extraArgs = ["-L" "root" "-f"];
              subvolumes = {
                "@root" = {
                  mountpoint = "/";
                  mountOptions = [
                    "compress=zstd"
                    "noatime"
                  ];
                };
                "@persist" = {
                  mountpoint = "/persist";
                  mountOptions = [
                    "compress=zstd"
                    "noatime"
                  ];
                };
                "@nix" = {
                  mountpoint = "/nix";
                  mountOptions = [
                    "compress=zstd"
                    "noatime"
                  ];
                };
                "@swap" = lib.mkIf withSwap {
                  mountpoint = "/.swapvol";
                  swap.swapfile.size = "${swapSize}G";
                };
              };
            };
          };
        };
      };
    };
  };
}

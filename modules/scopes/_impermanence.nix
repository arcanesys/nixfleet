# Core impermanence — universal persist paths only.
# Scope-specific persist paths live in their respective scope modules.
# Returns { nixos, hmLinux } module attrsets.
# mkHost imports these directly; they self-activate via lib.mkIf.
{
  # --- NixOS module: system-level persistence + btrfs wipe ---
  nixos = {
    config,
    lib,
    ...
  }: let
    hS = config.hostSpec;
  in {
    # Note: impermanence NixOS module import is handled by mkHost.
    config = lib.mkIf hS.isImpermanent {
      environment.persistence."/persist/system" = {
        directories = [
          "/etc/nixos"
          "/etc/NetworkManager/system-connections"
          "/var/lib/systemd"
          "/var/lib/nixos"
          "/var/log"
        ];
        files = ["/etc/machine-id"];
      };

      # --- Ensure persist home has correct ownership ---
      system.activationScripts.persistHomeOwnership = {
        text = ''
          install -d -o ${hS.userName} -g users /persist/home/${hS.userName}
          if [ -d /persist/home/${hS.userName}/.keys ]; then
            chown -R ${hS.userName}:users /persist/home/${hS.userName}/.keys
          fi
        '';
        deps = [];
      };

      # --- Btrfs root wipe ---
      boot.initrd.postResumeCommands = lib.mkAfter ''
        mkdir /btrfs_tmp
        mount /dev/disk/by-label/root /btrfs_tmp
        if [[ -e /btrfs_tmp/@root ]]; then
            mkdir -p /btrfs_tmp/old_roots
            timestamp=$(date --date="@$(stat -c %Y /btrfs_tmp/@root)" "+%Y-%m-%-d_%H:%M:%S")
            mv /btrfs_tmp/@root "/btrfs_tmp/old_roots/$timestamp"
        fi
        delete_subvolume_recursively() {
            IFS=$'\n'
            for i in $(btrfs subvolume list -o "$1" | cut -f 9- -d ' '); do
                delete_subvolume_recursively "/btrfs_tmp/$i"
            done
            btrfs subvolume delete "$1"
        }
        for i in $(find /btrfs_tmp/old_roots/ -maxdepth 1 -mtime +30); do
            delete_subvolume_recursively "$i"
        done
        btrfs subvolume create /btrfs_tmp/@root
        umount /btrfs_tmp
      '';
      fileSystems."/persist".neededForBoot = true;
    };
  };

  # --- HM module: user-level persistence (Linux only) ---
  hmLinux = {
    lib,
    osConfig,
    ...
  }: let
    hS = osConfig.hostSpec;
  in {
    config = lib.mkIf hS.isImpermanent {
      home.persistence."/persist" = {
        hideMounts = true;
        directories = [
          # Keys (agenix-managed, not .ssh or .gnupg — those are ephemeral)
          ".keys"

          # Source code
          ".local/share/src"

          # Shell
          ".zplug"
          ".local/share/zsh"

          # GitHub CLI
          ".config/gh"

          # Neovim
          ".local/share/nvim"
          ".cache/nvim"

          # Tmux resurrect sessions
          ".cache/tmux"

          # Zoxide
          ".local/share/zoxide"

          # Nix state
          ".local/share/nix"
        ];
        files = [
          ".ssh/known_hosts"
        ];
      };
    };
  };
}

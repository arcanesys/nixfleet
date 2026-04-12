# Core Darwin module. Imported directly by mkHost.
{
  config,
  pkgs,
  lib,
  ...
}: let
  hS = config.hostSpec;
  dockCfg = config.local.dock;
  inherit (pkgs) dockutil;
in {
  # --- dock option module ---
  options.local.dock = {
    enable = lib.mkOption {
      type = lib.types.bool;
      description = "Whether to manage the macOS Dock from Nix.";
      default = true;
      example = false;
    };
    entries = lib.mkOption {
      description = ''
        Entries to place on the Dock. Fleet repos populate this list
        with the apps, folders, and URLs they want managed.
      '';
      type = lib.types.listOf (lib.types.submodule {
        options = {
          path = lib.mkOption {
            type = lib.types.str;
            description = "Filesystem path or URL of the dock entry.";
            example = "/Applications/Safari.app";
          };
          section = lib.mkOption {
            type = lib.types.enum ["apps" "others"];
            default = "apps";
            description = "Dock section this entry belongs to.";
          };
          options = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "Extra options forwarded to dockutil.";
          };
        };
      });
      default = [];
      example = lib.literalExpression ''
        [
          { path = "/Applications/Safari.app"; }
          { path = "/Applications/Terminal.app"; }
        ]
      '';
    };
  };

  config = {
    # --- nixpkgs ---
    nixpkgs.config = {
      allowUnfree = true;
      allowBroken = false;
      allowInsecure = false;
      allowUnsupportedSystem = true;
    };

    # --- nix (Determinate install compatible) ---
    nix = {
      package = pkgs.nix;
      enable = false;
      settings = {
        trusted-users = [
          "@admin"
          "${hS.userName}"
        ];
        substituters = [
          "https://nix-community.cachix.org"
          "https://cache.nixos.org"
        ];
        trusted-public-keys = [
          "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
          "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
        ];
        auto-optimise-store = true;
      };
      extraOptions = ''
        experimental-features = nix-command flakes
      '';
    };

    # --- programs ---
    programs.zsh = {
      enable = true;
      enableCompletion = false;
    };

    # --- user ---
    users.users.${hS.userName} = {
      name = "${hS.userName}";
      home = "${hS.home}";
      isHidden = false;
      shell = pkgs.zsh;
    };

    # --- sudo (TouchID) ---
    security.pam.services.sudo_local.touchIdAuth = true;
    environment = {
      etc."pam.d/sudo_local".text = ''
        # Managed by Nix Darwin
        auth       optional       ${pkgs.pam-reattach}/lib/pam/pam_reattach.so ignore_ssh
        auth       sufficient     pam_tid.so
      '';
    };

    # --- system defaults ---
    system = {
      stateVersion = 4;
      checks.verifyNixPath = false;
      primaryUser = "${hS.userName}";
      defaults = {
        NSGlobalDomain = {
          AppleShowAllExtensions = true;
          ApplePressAndHoldEnabled = false;
          KeyRepeat = 2;
          InitialKeyRepeat = 15;
          "com.apple.mouse.tapBehavior" = 1;
          "com.apple.sound.beep.feedback" = 0;
        };
        dock = {
          autohide = true;
          show-recents = false;
          launchanim = true;
          orientation = "bottom";
          tilesize = 48;
        };
        finder = {
          AppleShowAllExtensions = true;
          AppleShowAllFiles = true;
          ShowPathbar = true;
          _FXSortFoldersFirst = true;
          _FXShowPosixPathInTitle = false;
        };
        trackpad = {
          Clicking = true;
          TrackpadThreeFingerDrag = true;
        };
      };
    };

    hostSpec.isDarwin = true;

    # --- dock management ---
    system.activationScripts.postActivation.text = lib.mkIf dockCfg.enable (
      let
        normalize = path:
          if lib.hasSuffix ".app" path
          then path + "/"
          else path;
        entryURI = path:
          "file://"
          + (
            builtins.replaceStrings
            [" " "!" "\"" "#" "$" "%" "&" "'" "(" ")"]
            ["%20" "%21" "%22" "%23" "%24" "%25" "%26" "%27" "%28" "%29"]
            (normalize path)
          );
        wantURIs = lib.concatMapStrings (entry: "${entryURI entry.path}\n") dockCfg.entries;
        createEntries =
          lib.concatMapStrings (
            entry: "sudo -u ${hS.userName} ${dockutil}/bin/dockutil --no-restart --add '${entry.path}' --section ${entry.section} ${entry.options}\n"
          )
          dockCfg.entries;
      in ''
        echo >&2 "Setting up the Dock..."
        haveURIs="$(sudo -u ${hS.userName} ${dockutil}/bin/dockutil --list | ${pkgs.coreutils}/bin/cut -f2)"
        if ! diff -wu <(echo -n "$haveURIs") <(echo -n '${wantURIs}') >&2 ; then
          echo >&2 "Resetting Dock."
          sudo -u ${hS.userName} ${dockutil}/bin/dockutil --no-restart --remove all
          ${createEntries}
          sudo -u ${hS.userName} killall Dock
        else
          echo >&2 "Dock setup complete."
        fi
      ''
    );
  };
}

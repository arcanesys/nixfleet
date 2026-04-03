# Base packages — truly universal tools for ALL hosts.
# Dev, graphical, and media packages belong in their respective scopes.
# Tool configs are managed by HM (core/_home/) with catppuccin auto-theming.
# Returns { nixos, darwin, homeManager } module attrsets.
# mkHost imports these directly; they self-activate via lib.mkIf.
{
  nixos = {
    config,
    pkgs,
    lib,
    ...
  }: let
    hS = config.hostSpec;
  in {
    environment.systemPackages = with pkgs;
      lib.optionals (!hS.isMinimal) [
        unixtools.ifconfig
        unixtools.netstat
        xdg-utils
      ];
  };

  darwin = {pkgs, ...}: {
    environment.systemPackages = with pkgs; [dockutil mas];
  };

  homeManager = {
    config,
    pkgs,
    lib,
    ...
  }: let
    hS = config.hostSpec;
  in {
    home.packages = with pkgs;
      lib.optionals (!hS.isMinimal) [
        # Core CLI tools
        coreutils
        killall
        openssh
        wget
        age
        gnupg
        fastfetch
        gh

        # File tools (ripgrep replaces ack, git diff --color replaces colordiff)
        duf
        eza
        fd
        fzf
        jq
        procs
        ripgrep
        tldr
        tree
        yq

        # Nix system management
        home-manager
        nh
      ];
  };
}

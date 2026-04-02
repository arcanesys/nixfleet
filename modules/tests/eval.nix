# Tier C — Eval tests: assert config properties at evaluation time.
# Runs via `nix flake check` (--no-build skips VM tests, eval checks are instant).
# Each check is a `pkgs.runCommand` that fails if any assertion is false.
#
# NOTE: Fleet-specific hostSpec options (isDev, isGraphical, useHyprland, theme, etc.)
# are NOT tested here — those are declared by consuming fleets and tested there.
{self, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib;};
    mkEvalCheck = helpers.mkEvalCheck pkgs;
    nixosCfg = name: self.nixosConfigurations.${name}.config;
  in
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        # --- SSH hardening (core/nixos.nix) ---
        eval-ssh-hardening = let
          cfg = nixosCfg "web-02";
        in
          mkEvalCheck "ssh-hardening" [
            {
              check = cfg.services.openssh.settings.PermitRootLogin == "prohibit-password";
              msg = "PermitRootLogin is prohibit-password";
            }
            {
              check = cfg.services.openssh.settings.PasswordAuthentication == false;
              msg = "PasswordAuthentication is false";
            }
            {
              check = cfg.networking.firewall.enable;
              msg = "firewall is enabled";
            }
          ];

        # --- hostSpec defaults propagate ---
        eval-hostspec-defaults = let
          cfg = nixosCfg "web-01";
        in
          mkEvalCheck "hostspec-defaults" [
            {
              check = cfg.hostSpec.userName != "";
              msg = "web-01 should have userName set";
            }
            {
              check = cfg.hostSpec.hostName == "web-01";
              msg = "web-01 should have hostName set";
            }
          ];

        # --- userName override ---
        eval-username-override = let
          refUser = (nixosCfg "web-01").hostSpec.userName;
        in
          mkEvalCheck "username-override" [
            {
              check = refUser != "";
              msg = "web-01 should have userName from shared defaults";
            }
            {
              check = (nixosCfg "dev-01").hostSpec.userName != refUser;
              msg = "dev-01 should override userName (different from shared default)";
            }
          ];

        # --- Locale / timezone ---
        eval-locale-timezone = let
          cfg = nixosCfg "web-01";
        in
          mkEvalCheck "locale-timezone" [
            {
              check = cfg.time.timeZone != "";
              msg = "web-01 should have timezone set";
            }
            {
              check = cfg.i18n.defaultLocale != "";
              msg = "web-01 should have locale set";
            }
            {
              check = cfg.console.keyMap != "";
              msg = "web-01 should have keyboard layout set";
            }
          ];

        # --- SSH authorized keys ---
        eval-ssh-authorized = let
          cfg = nixosCfg "web-01";
          userName = cfg.hostSpec.userName;
        in
          mkEvalCheck "ssh-authorized" [
            {
              check = builtins.length cfg.users.users.${userName}.openssh.authorizedKeys.keys > 0;
              msg = "web-01 should have SSH authorized keys";
            }
            {
              check = builtins.length cfg.users.users.root.openssh.authorizedKeys.keys > 0;
              msg = "web-01 root should have SSH authorized keys";
            }
          ];

        # --- Password file options exist ---
        eval-password-files = let
          cfg = nixosCfg "web-01";
        in
          mkEvalCheck "password-files" [
            {
              check = cfg.hostSpec ? hashedPasswordFile;
              msg = "hostSpec should have hashedPasswordFile option";
            }
            {
              check = cfg.hostSpec ? rootHashedPasswordFile;
              msg = "hostSpec should have rootHashedPasswordFile option";
            }
          ];

        # --- Agent tags and health checks ---
        eval-agent-tags-health = let
          cfg = nixosCfg "agent-test";
        in
          mkEvalCheck "agent-tags-health" [
            {
              check = cfg.systemd.services.nixfleet-agent.environment.NIXFLEET_TAGS == "web,production";
              msg = "agent-test should have NIXFLEET_TAGS set to web,production";
            }
            {
              check = cfg.environment.etc."nixfleet/health-checks.json".text != "";
              msg = "agent-test should have health-checks.json config file";
            }
          ];
      };
    };
}

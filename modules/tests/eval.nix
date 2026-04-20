# Tier C - Eval tests: assert config properties at evaluation time.
# Runs via `nix flake check` (--no-build skips VM tests, eval checks are instant).
# Each check is a `pkgs.runCommand` that fails if any assertion is false.
#
# NOTE: Fleet-specific hostSpec options (isDev, isGraphical, useHyprland, theme, etc.)
# are NOT tested here - those are declared by consuming fleets and tested there.
{self, ...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }: let
    helpers = import ./_lib/helpers.nix {inherit lib pkgs;};
    mkEvalCheck = helpers.mkEvalCheck pkgs;
    nixosCfg = name: self.nixosConfigurations.${name}.config;
    darwinCfg = name: self.darwinConfigurations.${name}.config;
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
        # Keys come from operators scope: primary operator -> user keys,
        # root authorized_keys sourced from nixfleet.operators.rootSshKeys
        # (via core/_nixos.nix) - independent of operator accounts.
        eval-ssh-authorized = let
          cfg = nixosCfg "web-01";
          userName = cfg.hostSpec.userName;
        in
          mkEvalCheck "ssh-authorized" [
            {
              check = builtins.length cfg.users.users.${userName}.openssh.authorizedKeys.keys > 0;
              msg = "web-01 primary operator should have SSH authorized keys";
            }
            {
              check = builtins.length cfg.users.users.root.openssh.authorizedKeys.keys > 0;
              msg = "web-01 root should have SSH authorized keys from rootSshKeys";
            }
          ];

        # --- Password file options exist ---
        # hashedPasswordFile moved to operators scope; only rootHashedPasswordFile
        # remains on hostSpec (root is not an operator).
        eval-password-files = let
          cfg = nixosCfg "web-01";
        in
          mkEvalCheck "password-files" [
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

        # --- Agent metrics port in ExecStart ---
        eval-agent-metrics = let
          cfg = nixosCfg "agent-test";
        in
          mkEvalCheck "agent-metrics" [
            {
              check = builtins.match ".*--metrics-port.*" cfg.systemd.services.nixfleet-agent.serviceConfig.ExecStart != null;
              msg = "agent-test should have --metrics-port in ExecStart";
            }
            {
              check = builtins.elem 9101 cfg.networking.firewall.allowedTCPPorts;
              msg = "agent-test should have metrics port 9101 in firewall";
            }
          ];

        # --- Secrets: resolved paths on server (host key only) ---
        eval-secrets-server = let
          cfg = nixosCfg "secrets-test";
        in
          mkEvalCheck "secrets-server" [
            {
              check = cfg.nixfleet.secrets.enable;
              msg = "secrets-test should have secrets scope enabled";
            }
            {
              check = cfg.nixfleet.secrets.resolvedIdentityPaths == ["/etc/ssh/ssh_host_ed25519_key"];
              msg = "server should have only host key in resolvedIdentityPaths";
            }
            {
              check = !cfg.nixfleet.secrets.identityPaths.enableUserKey;
              msg = "server should have enableUserKey = false";
            }
          ];

        # --- Secrets: resolved paths on workstation (host key + user key) ---
        eval-secrets-workstation = let
          cfg = nixosCfg "infra-test";
        in
          mkEvalCheck "secrets-workstation" [
            {
              check = builtins.length cfg.nixfleet.secrets.resolvedIdentityPaths == 2;
              msg = "workstation should have 2 identity paths (host key + user key)";
            }
            {
              check = builtins.head cfg.nixfleet.secrets.resolvedIdentityPaths == "/etc/ssh/ssh_host_ed25519_key";
              msg = "first identity path should be host key";
            }
          ];

        # --- Backup: option defaults ---
        eval-backup-defaults = let
          cfg = nixosCfg "infra-test";
        in
          mkEvalCheck "backup-defaults" [
            {
              check = cfg.nixfleet.backup.enable;
              msg = "infra-test should have backup enabled";
            }
            {
              check = cfg.nixfleet.backup.retention.daily == 7;
              msg = "retention.daily should default to 7";
            }
            {
              check = cfg.nixfleet.backup.retention.weekly == 4;
              msg = "retention.weekly should default to 4";
            }
            {
              check = cfg.nixfleet.backup.retention.monthly == 6;
              msg = "retention.monthly should default to 6";
            }
            {
              check = cfg.nixfleet.backup.paths == ["/persist"];
              msg = "backup paths should default to /persist";
            }
            {
              check = cfg.nixfleet.backup.schedule == "*-*-* 03:00:00";
              msg = "infra-test should have custom schedule";
            }
          ];

        # --- Monitoring: collector defaults ---
        eval-monitoring-defaults = let
          cfg = nixosCfg "infra-test";
        in
          mkEvalCheck "monitoring-defaults" [
            {
              check = cfg.nixfleet.monitoring.nodeExporter.enable;
              msg = "infra-test should have node exporter enabled";
            }
            {
              check = builtins.elem "systemd" cfg.nixfleet.monitoring.nodeExporter.enabledCollectors;
              msg = "systemd collector should be enabled";
            }
            {
              check = builtins.elem "cpu" cfg.nixfleet.monitoring.nodeExporter.enabledCollectors;
              msg = "cpu collector should be enabled";
            }
            {
              check = builtins.elem "textfile" cfg.nixfleet.monitoring.nodeExporter.disabledCollectors;
              msg = "textfile collector should be disabled";
            }
            {
              check = builtins.elem "wifi" cfg.nixfleet.monitoring.nodeExporter.disabledCollectors;
              msg = "wifi collector should be disabled";
            }
          ];

        # --- Firewall: nftables enabled on non-minimal ---
        eval-firewall-nftables = let
          cfg = nixosCfg "web-01";
        in
          mkEvalCheck "firewall-nftables" [
            {
              check = cfg.networking.nftables.enable;
              msg = "non-minimal host should have nftables enabled";
            }
            {
              check = cfg.networking.firewall.logRefusedConnections;
              msg = "non-minimal host should log refused connections";
            }
          ];

        # --- Firewall: minimal host should NOT have nftables forced ---
        eval-firewall-minimal = let
          cfg = nixosCfg "edge-01";
        in
          mkEvalCheck "firewall-minimal" [
            {
              check = !cfg.networking.nftables.enable;
              msg = "minimal host should not have nftables forced by firewall scope";
            }
          ];

        # --- Cache server: port, firewall, signing key ---
        eval-cache-server = let
          cfg = nixosCfg "cache-test";
        in
          mkEvalCheck "cache-server" [
            {
              check = cfg.services.nixfleet-cache-server.enable;
              msg = "cache-test should have cache server enabled";
            }
            {
              check = builtins.elem 5000 cfg.networking.firewall.allowedTCPPorts;
              msg = "cache-test should have port 5000 in firewall";
            }
            {
              check = cfg.services.nixfleet-cache-server.signingKeyFile == "/run/secrets/cache-signing-key";
              msg = "cache-test should have signing key file set";
            }
          ];

        # --- Cache client: substituters and trusted keys ---
        eval-cache = let
          cfg = nixosCfg "cache-test";
        in
          mkEvalCheck "cache" [
            {
              check = cfg.services.nixfleet-cache.enable;
              msg = "cache-test should have cache client enabled";
            }
            {
              check = builtins.elem "http://localhost:5000" cfg.nix.settings.substituters;
              msg = "cache client should add cache URL to substituters";
            }
            {
              check = builtins.elem "cache-test:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" cfg.nix.settings.trusted-public-keys;
              msg = "cache client should add public key to trusted-public-keys";
            }
          ];

        # --- MicroVM host: bridge, IP forwarding ---
        eval-microvm-host = let
          cfg = nixosCfg "microvm-test";
        in
          mkEvalCheck "microvm-host" [
            {
              check = cfg.services.nixfleet-microvm-host.enable;
              msg = "microvm-test should have microvm host enabled";
            }
            {
              check = cfg.services.nixfleet-microvm-host.bridge.name == "nixfleet-br0";
              msg = "microvm-test should have default bridge name";
            }
            {
              check = cfg.boot.kernel.sysctl."net.ipv4.ip_forward" == 1;
              msg = "microvm-test should have IP forwarding enabled";
            }
            {
              check = cfg.networking.nat.enable;
              msg = "microvm-test should have NAT enabled";
            }
            {
              check = cfg.services.dnsmasq.enable;
              msg = "microvm-test should have dnsmasq DHCP enabled";
            }
          ];

        # --- Backup restic: ExecStart, packages ---
        eval-backup-restic = let
          cfg = nixosCfg "backup-restic-test";
        in
          mkEvalCheck "backup-restic" [
            {
              check = cfg.nixfleet.backup.enable;
              msg = "backup-restic-test should have backup enabled";
            }
            {
              check = cfg.nixfleet.backup.backend == "restic";
              msg = "backup-restic-test should have restic backend";
            }
            {
              check = cfg.nixfleet.backup.restic.repository == "/mnt/backup/restic";
              msg = "backup-restic-test should have restic repository set";
            }
            {
              check = builtins.any (p: (p.pname or "") == "restic") cfg.environment.systemPackages;
              msg = "backup-restic-test should have restic in system packages";
            }
          ];

        # --- Darwin agent: launchd service present ---
        eval-darwin-agent-launchd = let
          cfg = darwinCfg "darwin-agent-test";
        in
          mkEvalCheck "darwin-agent-launchd" [
            {
              check = cfg.launchd.daemons.nixfleet-agent.serviceConfig.Label == "com.nixfleet.agent";
              msg = "darwin-agent-test should have launchd daemon with correct label";
            }
            {
              check = cfg.launchd.daemons.nixfleet-agent.serviceConfig.KeepAlive == true;
              msg = "darwin-agent-test should have KeepAlive enabled";
            }
          ];

        # --- Darwin agent: health config written ---
        eval-darwin-agent-health = let
          cfg = darwinCfg "darwin-agent-test";
        in
          mkEvalCheck "darwin-agent-health" [
            {
              check = cfg.environment.etc."nixfleet/health-checks.json".text != "";
              msg = "darwin-agent-test should have health-checks.json config file";
            }
          ];

        # --- Darwin agent: tags in environment ---
        eval-darwin-agent-tags = let
          cfg = darwinCfg "darwin-agent-test";
        in
          mkEvalCheck "darwin-agent-tags" [
            {
              check = cfg.launchd.daemons.nixfleet-agent.serviceConfig.EnvironmentVariables.NIXFLEET_TAGS == "workstation,darwin";
              msg = "darwin-agent-test should have NIXFLEET_TAGS set";
            }
          ];

        # --- Darwin hostSpec: isDarwin flag ---
        eval-darwin-hostspec = let
          cfg = darwinCfg "darwin-agent-test";
        in
          mkEvalCheck "darwin-hostspec" [
            {
              check = cfg.hostSpec.isDarwin == true;
              msg = "darwin-agent-test should have isDarwin set to true";
            }
            {
              check = cfg.hostSpec.hostName == "darwin-agent-test";
              msg = "darwin-agent-test should have correct hostName";
            }
          ];
      };
    };
}

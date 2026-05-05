{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;
  nixfleet-agent = cfg.package;

  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;
in {
  imports = [./_agent-options.nix];

  config = lib.mkIf cfg.enable {
    # FOOTGUN: `.text` (not `.source`); ships content in closure, not a symlink to a flake path absent on the deployed host.
    environment.etc."nixfleet/agent/trust.json".text = builtins.readFile trustJson;

    system.activationScripts.preActivation.text = ''
      mkdir -p /var/lib/nixfleet
      mkdir -p ${lib.escapeShellArg cfg.stateDir}
      chmod 0700 ${lib.escapeShellArg cfg.stateDir}
      # Activate-script log file used by the agent's setsid-detached
      # `<store>/activate` invocation (see crates/nixfleet-agent/
      # src/activation.rs::fire_switch_darwin). Touched here so the
      # OpenOptions(append) in attach_activate_log can succeed on
      # first boot.
      install -m 0644 /dev/null /var/log/nixfleet-activate.log 2>/dev/null || true
    '';

    # GOTCHA: force-restart after activation; launchd KeepAlive doesn't fire when only closure changes (plist bytes unchanged).
    system.activationScripts.postActivation.text = lib.mkAfter ''
      echo "restarting nixfleet agent..." >&2
      launchctl kickstart -k system/com.nixfleet.agent 2>/dev/null || true
    '';

    launchd.daemons.nixfleet-agent = {
      serviceConfig = {
        Label = "com.nixfleet.agent";

        # GOTCHA: 15s sleep gives NTP + agenix time at boot; exec replaces sh so launchd tracks agent PID.
        ProgramArguments = let
          args = lib.concatStringsSep " " (import ./_agent-args.nix {
            inherit lib cfg;
            package = nixfleet-agent;
          });
        in ["/bin/sh" "-c" "sleep 15 && exec ${args}"];

        KeepAlive = true;
        RunAtLoad = true;
        ThrottleInterval = 10;
        ExitTimeOut = 10;

        StandardOutPath = "/var/log/nixfleet-agent.log";
        StandardErrorPath = "/var/log/nixfleet-agent.log";

        # Created by preActivation; launchd exits with I/O error otherwise.
        WorkingDirectory = "/var/lib/nixfleet";

        EnvironmentVariables =
          {
            XDG_CACHE_HOME = "/var/lib/nixfleet/.cache";
            # Cover Determinate Nix + standard nix-darwin profile paths.
            PATH = "/nix/var/nix/profiles/default/bin:/run/current-system/sw/bin:/usr/bin:/bin";
          }
          // lib.optionalAttrs (cfg.tags != []) {
            NIXFLEET_TAGS = lib.concatStringsSep "," cfg.tags;
          };
      };
    };
  };
}

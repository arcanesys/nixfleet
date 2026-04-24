# NixOS service module for the NixFleet control plane (Phase 2: read-only
# reconciler runner).
#
# Phase 2 shape: one-shot binary launched on a systemd timer. Reads
# fleet.resolved.json + signature + trust.json + observed.json, verifies,
# reconciles, prints the action plan as JSON lines to the journal, exits.
# No actions taken on the fleet — the reconciler brain runs against
# simulated observed state until Phase 3 wires real agent check-ins.
#
# Auto-included by mkHost (disabled by default). Enable on the
# coordinator host (typically the M70q) only.
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-control-plane;
  nixfleet-control-plane = inputs.self.packages.${pkgs.system}.nixfleet-control-plane;

  # Shared trust.json payload — see ./_trust-json.nix for shape rationale
  # and the orgRootKey ed25519 promotion that matches proto::TrustConfig.
  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;

  # First-deploy bootstrap for observed.json — laid down via
  # systemd-tmpfiles `C+` (copy only if path does not exist) so the
  # reconciler's first tick has a parseable file even before the
  # operator has hand-written one. Operator can edit
  # `cfg.observedPath` afterwards to populate channel refs / host
  # state / active rollouts; subsequent rebuilds will not overwrite.
  initialObservedJson = pkgs.writers.writeJSON "observed-initial.json" {
    channelRefs = {};
    lastRolledRefs = {};
    hostState = {};
    activeRollouts = [];
  };
in {
  options.services.nixfleet-control-plane = {
    enable = lib.mkEnableOption "NixFleet Phase 2 reconciler runner (read-only timer)";

    artifactPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json";
      description = ''
        Path to the canonical fleet.resolved.json bytes (the file CI
        signed). Operator is responsible for keeping this path
        up-to-date with the fleet repo's HEAD — typically a separate
        timer that pulls the fleet repo into
        `/var/lib/nixfleet-cp/fleet/`. The CP module does not pull
        git itself.
      '';
    };

    signaturePath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json.sig";
      description = "Path to the raw signature bytes paired with `artifactPath`.";
    };

    observedPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/observed.json";
      description = ''
        Path to the JSON file holding observed fleet state (channel refs,
        host states, active rollouts) — shape per
        `nixfleet_reconciler::Observed`. In Phase 2 the operator
        hand-writes this; Phase 3 swaps to a SQLite-backed projection
        updated by agent check-ins. Examples in
        `crates/nixfleet-control-plane/fixtures/observed/`.
      '';
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/cp/trust.json";
      description = ''
        Path to the trust-root JSON file (see
        docs/trust-root-flow.md §3.4). Materialised by this module
        from `config.nixfleet.trust` via environment.etc.
      '';
    };

    freshnessWindowMinutes = lib.mkOption {
      type = lib.types.ints.positive;
      default = 1440;
      description = ''
        Maximum age (minutes) of `meta.signedAt` accepted by
        `verify_artifact`. Match the operator-declared channel
        `freshnessWindow` in fleet.nix when in doubt; default is 24h.
      '';
    };

    tickIntervalMinutes = lib.mkOption {
      type = lib.types.ints.positive;
      default = 5;
      description = ''
        Minutes between reconcile ticks. In Phase 2 the runner is
        side-effect-free (just emits a plan), so cadence is purely an
        observability knob.
      '';
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      environment.etc."nixfleet/cp/trust.json".source = trustJson;

      systemd.services.nixfleet-control-plane = {
        description = "NixFleet Phase 2 reconciler runner (one-shot)";
        after = ["network.target"];
        unitConfig.ConditionPathExists = cfg.artifactPath;
        serviceConfig = {
          Type = "oneshot";
          ExecStart = lib.concatStringsSep " " [
            "${nixfleet-control-plane}/bin/nixfleet-control-plane"
            "--artifact"
            (lib.escapeShellArg cfg.artifactPath)
            "--signature"
            (lib.escapeShellArg cfg.signaturePath)
            "--trust-file"
            (lib.escapeShellArg (toString cfg.trustFile))
            "--observed"
            (lib.escapeShellArg cfg.observedPath)
            "--freshness-window-secs"
            (toString (cfg.freshnessWindowMinutes * 60))
          ];
          StateDirectory = "nixfleet-cp";

          # Read-only against everything except its own state dir;
          # writes nothing of consequence (output is on the journal).
          DynamicUser = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          NoNewPrivileges = true;
          RestrictAddressFamilies = ["AF_UNIX"];
          # Reconciler is pure CPU + file reads — no network.
          PrivateNetwork = true;
          # Keeps the StateDirectory writable for future Phase 3 use;
          # Phase 2 itself doesn't need to write there.
          ReadWritePaths = ["/var/lib/nixfleet-cp"];
        };
      };

      # First-deploy auto-bootstrap of observed.json. tmpfiles `C+ … - <src>`
      # copies from `<src>` only if `<target>` does not already exist —
      # subsequent rebuilds leave operator-written content untouched.
      systemd.tmpfiles.rules = [
        "d /var/lib/nixfleet-cp 0755 root root -"
        "C+ ${cfg.observedPath} 0644 root root - ${initialObservedJson}"
      ];

      systemd.timers.nixfleet-control-plane = {
        description = "NixFleet Phase 2 reconciler runner (timer)";
        wantedBy = ["timers.target"];
        timerConfig = {
          OnBootSec = "30s";
          OnUnitActiveSec = "${toString cfg.tickIntervalMinutes}m";
          AccuracySec = "10s";
          # Stagger across hosts in case multiple coordinators ever exist.
          RandomizedDelaySec = "30s";
        };
      };
    })

    # Impermanence: persist CP state across reboots.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet-cp"];
    })
  ];
}

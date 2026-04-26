# NixOS service module for the NixFleet control plane.
#
# Phase 3 PR-1: long-running TLS server. The binary's `serve`
# subcommand runs forever, accepts mTLS-authenticated connections (PR-2
# adds the verifier), and ticks an internal reconcile loop every 30s.
# This replaces Phase 2's oneshot+timer pair — `Type=oneshot` →
# `Type=simple`, `systemd.timers.nixfleet-control-plane` is dropped.
# The `tick` subcommand on the binary preserves Phase 2's CLI contract
# for tests and ad-hoc operator runs.
#
# Auto-included by mkHost (disabled by default). Enable on the
# coordinator host (typically lab) only.
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
  # systemd-tmpfiles `C` (copy only if path does not exist) so the
  # reconciler's first tick has a parseable file even before the
  # operator has hand-written one. PR-4 swaps this for an in-memory
  # projection from agent check-ins; this stays as the offline
  # dev/test fallback.
  initialObservedJson = pkgs.writers.writeJSON "observed-initial.json" {
    channelRefs = {};
    lastRolledRefs = {};
    hostState = {};
    activeRollouts = [];
  };

  # Parse the listen address into HOST:PORT for the firewall rule.
  listenPort = lib.toInt (lib.last (lib.splitString ":" cfg.listen));
in {
  options.services.nixfleet-control-plane = {
    enable = lib.mkEnableOption "NixFleet control plane (Phase 3: long-running TLS server)";

    listen = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0:8080";
      example = "0.0.0.0:8080";
      description = ''
        HOST:PORT the control plane listens on. Default 8080 per spec
        D3 — port < 1024 would require CAP_NET_BIND_SERVICE; 443
        collides with operator-facing services on lab.
      '';
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Open the listen port in the system firewall. Defaults to
        false because lab is on Tailscale; production deploys may
        want this true once the perimeter posture is reviewed.
      '';
    };

    tls = {
      cert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/cp-cert";
        description = ''
          Path to the TLS server certificate PEM file. Wired by
          `fleet/modules/nixfleet/tls.nix` to the agenix-decrypted
          `cp-cert` path. Required when `enable = true`; the
          assertion at config-time enforces this.
        '';
      };

      key = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/cp-key";
        description = ''
          Path to the TLS server private key PEM file (agenix-
          decrypted `cp-key` path). Required when `enable = true`.
        '';
      };

      clientCa = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/etc/nixfleet/fleet-ca.pem";
        description = ''
          Path to the client CA PEM file. When set, the server
          requires verified client certs (mTLS). PR-1 leaves this
          optional — the server starts in TLS-only mode if unset
          and logs a warning. PR-2 onwards sets this as part of
          standard deploys; production hosts should always have it.
        '';
      };
    };

    artifactPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json";
      description = ''
        Path to the canonical fleet.resolved.json bytes (the file CI
        signed). Operator is responsible for keeping this path
        up-to-date with the fleet repo's HEAD — typically a separate
        timer that pulls the fleet repo into
        `/var/lib/nixfleet-cp/fleet/`. The CP module does not pull
        git itself in PR-1; PR-4 adds in-process Forgejo polling.
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
        Path to the JSON file holding observed fleet state — shape
        per `nixfleet_reconciler::Observed`. Phase 2 + PR-1: hand-
        written by the operator (auto-bootstrapped to an empty
        skeleton on first deploy via systemd-tmpfiles). PR-4 swaps
        the live in-memory projection from agent check-ins; this
        path remains as the offline dev/test fallback.
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

    # PR-5: cert issuance (enroll + renew). The CP holds the fleet
    # CA private key online — see nixfleet issue #41 for the deferred
    # TPM-bound replacement. fleet/modules/nixfleet/tls.nix wires
    # these to agenix-decrypted paths.
    fleetCaCert = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/etc/nixfleet/fleet-ca.pem";
      description = ''
        Fleet CA cert path. Used by issuance for chain assembly
        (clientAuth EKU agent certs). Typically the same path as
        `tls.clientCa`.
      '';
    };

    fleetCaKey = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/agenix/fleet-ca-key";
      description = ''
        Fleet CA private key path (agenix-decrypted). Used to sign
        agent certs in /v1/enroll and /v1/agent/renew. **Online on
        the CP — see nixfleet issue #41.**
      '';
    };

    auditLogPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/issuance.log";
      description = ''
        JSON-lines audit log of every cert issuance (enroll | renew).
        Best-effort writes; failure logs a warn but doesn't block
        issuance.
      '';
    };

    # PR-4: Forgejo channel-refs poll. When set, the CP polls
    # /api/v1/repos/{owner}/{repo}/contents/{artifactPath} every 60s
    # and refreshes the in-memory channel-refs cache. Phase 4 may
    # extend this with a sibling poll for the .sig file (verify-on-
    # load) — for now the CP trusts the authenticated TLS channel
    # to Forgejo + Forgejo's RBAC.
    forgejo = {
      baseUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.lab.internal";
        description = ''
          Forgejo base URL (no trailing slash). When null, channel-
          refs polling is disabled and the CP falls back to the
          file-backed observed.json for channel-refs.
        '';
      };

      owner = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "abstracts33d";
        description = "Forgejo repo owner for the fleet repo.";
      };

      repo = lib.mkOption {
        type = lib.types.str;
        default = "fleet";
        description = "Forgejo repo name (default `fleet`).";
      };

      artifactPath = lib.mkOption {
        type = lib.types.str;
        default = "releases/fleet.resolved.json";
        description = "Path inside the repo to fleet.resolved.json.";
      };

      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/cp-forgejo-token";
        description = ''
          Path to a file containing a Forgejo API token with read
          access to the fleet repo. Wired by fleet/modules/secrets/
          nixos.nix to an agenix-decrypted path. Read on each poll
          so token rotation propagates without restart.
        '';
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = builtins.match ".*:[0-9]+" cfg.listen != null;
          message = ''
            services.nixfleet-control-plane.listen must be in HOST:PORT format
            (e.g. "0.0.0.0:8080"), got: "${cfg.listen}"
          '';
        }
        {
          assertion = (cfg.tls.cert != null) && (cfg.tls.key != null);
          message = ''
            services.nixfleet-control-plane requires both tls.cert and tls.key
            to be set when enabled. Wire them through agenix in fleet/modules/
            nixfleet/tls.nix (see commit 3d68ed8^ for the historical shape, or
            the prep PR re-introducing it for Phase 3).
          '';
        }
      ];

      environment.etc."nixfleet/cp/trust.json".source = trustJson;

      systemd.services.nixfleet-control-plane = {
        description = "NixFleet control plane (long-running TLS server)";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];
        unitConfig.ConditionPathExists = cfg.artifactPath;

        serviceConfig = {
          Type = "simple";
          ExecStart = lib.concatStringsSep " " (
            [
              "${nixfleet-control-plane}/bin/nixfleet-control-plane"
              "serve"
              "--listen"
              (lib.escapeShellArg cfg.listen)
              "--tls-cert"
              (lib.escapeShellArg cfg.tls.cert)
              "--tls-key"
              (lib.escapeShellArg cfg.tls.key)
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
            ]
            ++ lib.optionals (cfg.tls.clientCa != null) [
              "--client-ca"
              (lib.escapeShellArg cfg.tls.clientCa)
            ]
            ++ lib.optionals (cfg.fleetCaCert != null) [
              "--fleet-ca-cert"
              (lib.escapeShellArg cfg.fleetCaCert)
            ]
            ++ lib.optionals (cfg.fleetCaKey != null) [
              "--fleet-ca-key"
              (lib.escapeShellArg cfg.fleetCaKey)
            ]
            ++ [
              "--audit-log"
              (lib.escapeShellArg cfg.auditLogPath)
            ]
            ++ lib.optionals
            (
              cfg.forgejo.baseUrl != null
              && cfg.forgejo.owner != null
              && cfg.forgejo.tokenFile != null
            ) [
              "--forgejo-base-url"
              (lib.escapeShellArg cfg.forgejo.baseUrl)
              "--forgejo-owner"
              (lib.escapeShellArg cfg.forgejo.owner)
              "--forgejo-repo"
              (lib.escapeShellArg cfg.forgejo.repo)
              "--forgejo-artifact-path"
              (lib.escapeShellArg cfg.forgejo.artifactPath)
              "--forgejo-token-file"
              (lib.escapeShellArg cfg.forgejo.tokenFile)
            ]
          );
          Restart = "always";
          RestartSec = 10;
          StateDirectory = "nixfleet-cp";

          # Hardening — same posture as v0.1's CP module (tag v0.1.1).
          # Network access is required (TLS listener), so PrivateNetwork
          # from Phase 2 is dropped. ProtectSystem=strict is fine since
          # the server reads from /etc + /var/lib + /run/agenix and only
          # writes to its StateDirectory.
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          NoNewPrivileges = true;
          ReadWritePaths = ["/var/lib/nixfleet-cp"];
        };
      };

      # First-deploy auto-bootstrap of observed.json. tmpfiles type `C`
      # (without the `+` modifier) copies from the seed path only if
      # the target does not already exist — operator edits to
      # observed.json survive rebuilds.
      systemd.tmpfiles.rules = [
        "d /var/lib/nixfleet-cp 0755 root root -"
        "C ${cfg.observedPath} 0644 root root - ${initialObservedJson}"
      ];

      networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [listenPort];
    })

    # Impermanence: persist CP state across reboots.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet-cp"];
    })
  ];
}

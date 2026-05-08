{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-control-plane;
  nixfleet-control-plane = cfg.package;

  trustConfig = import ./_trust-json.nix {trust = config.nixfleet.trust;};
  trustJson = pkgs.writers.writeJSON "trust.json" trustConfig;

  # LOADBEARING: single source of truth for sidecar (artifact + signature + token) wiring; tokenFallback covers shared upstream tokens.
  mkPollingSource = {
    artifactFlag,
    signatureFlag,
    tokenFlag,
    artifact,
    signature,
    tokenFile,
    tokenFallback ? null,
  }: let
    enabled = artifact != null && signature != null;
    effectiveToken =
      if tokenFile != null
      then tokenFile
      else tokenFallback;
  in
    lib.optionals enabled (
      [
        artifactFlag
        (lib.escapeShellArg artifact)
        signatureFlag
        (lib.escapeShellArg signature)
      ]
      ++ lib.optionals (effectiveToken != null) [
        tokenFlag
        (lib.escapeShellArg effectiveToken)
      ]
    );

  # GOTCHA: systemd-tmpfiles `C` copies only if path absent.
  initialObservedJson = pkgs.writers.writeJSON "observed-initial.json" {
    channelRefs = {};
    lastRolledRefs = {};
    hostState = {};
    activeRollouts = [];
  };

  listenPort = lib.toInt (lib.last (lib.splitString ":" cfg.listen));

  artifactDir = builtins.dirOf cfg.artifactPath;
in {
  options.services.nixfleet-control-plane = {
    enable = lib.mkEnableOption "NixFleet control plane (long-running TLS server)";

    package = lib.mkOption {
      type = lib.types.package;
      default = inputs.self.packages.${pkgs.system}.nixfleet-control-plane;
      defaultText = lib.literalExpression "inputs.self.packages.\${pkgs.system}.nixfleet-control-plane";
      description = ''
        The control-plane package that provides
        `bin/nixfleet-control-plane`. Defaults to the flake's
        crane-built package; tests and pinned-version deploys override
        with their own derivation. Standard NixOS `services.<x>.package`
        escape hatch — accepted as-is, no further resolution.
      '';
    };

    listen = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0:8080";
      example = "0.0.0.0:8080";
      description = ''
        HOST:PORT the control plane listens on. Default 8080 per spec
        D3 — port < 1024 would require CAP_NET_BIND_SERVICE, and 443
        is typically already taken by a reverse proxy on the same host.
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
        example = "/run/secrets/cp-cert";
        description = ''
          Path to the TLS server certificate PEM file. Wired by
          the fleet's secrets backend to the decrypted `cp-cert`
          path. Required when `enable = true`; the assertion at
          config-time enforces this.
        '';
      };

      key = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-key";
        description = ''
          Path to the TLS server private key PEM file (decrypted
          `cp-key` path). Required when `enable = true`.
        '';
      };

      clientCa = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/etc/nixfleet/fleet-ca.pem";
        description = ''
          Path to the client CA PEM file. When set, the server
          requires verified client certs (mTLS). Optional — the
          server starts in TLS-only mode if unset and logs a warning.
          Standard deploys set this; production hosts should always
          have it.
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
        git itself; in-process Forgejo polling can refresh this
        cache automatically.
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
        per `nixfleet_reconciler::Observed`. Hand-written by the
        operator (auto-bootstrapped to an empty skeleton on first
        deploy via systemd-tmpfiles). The live in-memory projection
        from agent check-ins is preferred; this path remains as the
        offline dev/test fallback.
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

    strict = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        When `true`, the control plane refuses to start if any of the
        security-relevant flags are unset:

        - `tls.clientCa` (without it, mTLS verification is disabled and
          all `/v1/*` endpoints serve TLS-only — the `auth_cn` middleware
          falls through and identity-bound checks become no-ops).
        - `revocationsSource.{artifactUrl,signatureUrl}` (without them,
          revocations polling is silently disabled, so previously revoked
          certs become valid again after a CP rebuild — contradicts the
          §6 Phase 10 promise that CP-rebuild recovery preserves
          revocations).
        - `X-Nixfleet-Protocol` header on incoming requests (strict mode
          turns the missing-header forward-compat slack into a 400).

        Default `false` for v0.2 to preserve current behaviour. Strongly
        recommended for production; see #70 for the rationale.
      '';
    };

    confirmDeadlineSecs = lib.mkOption {
      type = lib.types.ints.positive;
      default = 360;
      description = ''
        Seconds the dispatch loop gives an agent to fetch + activate
        + confirm a target before the magic-rollback timer marks the
        pending row as `rolled-back`.

        Default 360s: agents activate via fire-and-forget (ADR-011,
        ~300s polling `/run/current-system` after the detached
        `systemd-run` is fired) plus 60s slack. Dropping below ~310s
        creates a chaos cascade — CP rolls back while the agent is
        still polling, agent eventually polls success, posts confirm,
        CP returns 410, agent triggers local rollback.

        Tune up for slow-link channels (large closures over residential
        uplinks); avoid tuning down without first lowering the
        agent-side poll budget. Wraps the binary's `--confirm-deadline-secs`.
      '';
    };

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
      example = "/run/secrets/fleet-ca-key";
      description = ''
        Fleet CA private key path (decrypted by the fleet's secrets
        backend). Used to sign agent certs in /v1/enroll and
        /v1/agent/renew via the file-backed `FileCaSigner`. Mutually
        exclusive with the `tpmCa*` options at runtime — when TPM
        flags are set, the CP picks `TpmCaSigner` and this path is
        ignored. Keep set during the Bundle C migration overlap so a
        revert (drop `tpmCa*` options) restores file-backed signing
        without re-deriving the legacy CA.
      '';
    };

    tpmCaPubkeyRaw = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/var/lib/nixfleet-tpm-keyslot/issuanceCA/pubkey.raw";
      description = ''
        Bundle C (nixfleet#41): path to the keyslots scope's
        `pubkey.raw` (64 bytes raw P-256 X||Y) for the issuance CA's
        TPM key. Setting this AND `tpmCaSignWrapper` switches issuance
        to TPM signing; `fleetCaKey` becomes unused. Pair with
        `fleetCaCert` pointing at the offline-root-signed issuance CA
        cert (typically `/etc/nixfleet/cp/issuance-ca.pem`).
      '';
    };

    tpmCaSignWrapper = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/current-system/sw/bin/tpm-sign-issuanceCA";
      description = ''
        Bundle C (nixfleet#41): path to the keyslots scope's
        `tpm-sign-<keyname>` shell wrapper. Set together with
        `tpmCaPubkeyRaw` to enable TPM-backed issuance.
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

    closureUpstream = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "http://localhost:8085";
      description = ''
        Attic upstream URL for closure-proxy forwarding. Ships
        narinfo forwarding (operator can curl
        `<cp>/v1/agent/closure/<hash>` and get the upstream's
        narinfo response). Full nar streaming is a follow-up.
      '';
    };

    rolloutsDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/var/lib/nixfleet-cp/releases/rollouts";
      description = ''
        Filesystem directory holding pre-signed rollout manifests
        produced by `nixfleet-release`. Required for the v0.2
        content-addressed dispatch path; without it agents refuse
        to act on every dispatched target.
      '';
    };

    dbPath = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = "/var/lib/nixfleet-cp/state.db";
      description = ''
        Path to the SQLite database. Default lives under
        StateDirectory so impermanent hosts can persist via
        environment.persistence (already declared below). Set to
        `null` to disable persistence — e.g. for dev/test or until
        the operator is ready for the full stateful CP.
      '';
    };

    channelRefsSource = {
      artifactUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/fleet.resolved.json";
        description = ''
          Fully-formed URL that yields the raw bytes of the canonical
          signed fleet.resolved.json. When null, channel-refs polling
          is disabled and the CP falls back to the file-backed
          observed.json. Must be set together with `signatureUrl`.
        '';
      };

      signatureUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/fleet.resolved.json.sig";
        description = ''
          Fully-formed URL that yields the raw bytes of the matching
          signature. The poll task fetches both files together and
          runs verify_artifact — this is what closes the GitOps loop
          (push → CI re-sign → poll picks up new closureHashes within
          ~60s, no CP redeploy).
        '';
      };

      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-channel-refs-token";
        description = ''
          Path to a file containing the upstream API token (sent as
          `Authorization: Bearer <token>`). Optional — leave null for
          public sources (e.g. unauthenticated raw URLs on a public
          forge or a plain HTTPS file server). Read on each poll so
          token rotation propagates without restart.
        '';
      };
    };

    revocationsSource = {
      artifactUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/revocations.json";
        description = ''
          Fully-formed URL that yields the raw bytes of the canonical
          signed `revocations.json`. When null, revocations polling
          is disabled. Must be set together with `signatureUrl`.
        '';
      };

      signatureUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/revocations.json.sig";
        description = ''
          Fully-formed URL that yields the raw bytes of the matching
          signature. Verified against the same `ciReleaseKey` trust
          roots as `fleet.resolved.json`.
        '';
      };

      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-revocations-token";
        description = ''
          Path to a file containing the upstream API token. Defaults
          to falling back on `channelRefsSource.tokenFile` when null
          since both artifacts typically live in the same upstream
          repo with the same auth scope. Set explicitly only if the
          two artifacts ship from different sources.
        '';
      };
    };

    # LOADBEARING: URL templates contain literal `{rolloutId}`; CP substitutes per fetch.
    rolloutsSource = {
      artifactUrlTemplate = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/rollouts/{rolloutId}.json";
        description = ''
          URL template for HTTP-fetched rollout manifests. Must
          contain the literal `{rolloutId}` token; the CP substitutes
          the requested rolloutId at fetch time. When null, manifest
          distribution falls back to `rolloutsDir`. Must be set
          together with `signatureUrlTemplate`.
        '';
      };

      signatureUrlTemplate = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://git.example.com/myorg/myfleet/raw/branch/main/releases/rollouts/{rolloutId}.json.sig";
        description = ''
          Signature URL template (same `{rolloutId}` substitution as
          the artifact template). Both required to enable HTTP fetch.
          Verified by the agent against the same `ciReleaseKey` trust
          roots as `fleet.resolved.json`.
        '';
      };

      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/cp-rollouts-token";
        description = ''
          Path to a file containing the upstream API token. Defaults
          to falling back on `channelRefsSource.tokenFile` when null
          (the typical case: one Forgejo instance, one access token,
          all three sidecars share it).
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
            to be set when enabled. Wire them through your secrets backend.
          '';
        }
      ];

      # FOOTGUN: prefix checks (not regex); Nix builtins.match uses POSIX ERE which rejects `\[` outside a bracket expression.
      warnings = let
        listen = cfg.listen;
        isLoopback =
          lib.hasPrefix "127." listen
          || lib.hasPrefix "localhost:" listen
          || lib.hasPrefix "[::1]:" listen;
      in
        lib.optional (!cfg.strict && !isLoopback) ''
          services.nixfleet-control-plane.listen = "${cfg.listen}" exposes
          the CP beyond loopback while strict = false. Consider setting
          strict = true so missing --client-ca / revocations / protocol-
          header flags fail loudly rather than silently degrading the
          security posture (#70).
        '';

      environment.etc."nixfleet/cp/trust.json".source = trustJson;

      systemd.services.nixfleet-control-plane = {
        description = "NixFleet control plane (long-running TLS server)";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];

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
              "--confirm-deadline-secs"
              (toString cfg.confirmDeadlineSecs)
            ]
            ++ lib.optionals cfg.strict ["--strict"]
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
            ++ lib.optionals (cfg.tpmCaPubkeyRaw != null) [
              "--tpm-ca-pubkey-raw"
              (lib.escapeShellArg cfg.tpmCaPubkeyRaw)
            ]
            ++ lib.optionals (cfg.tpmCaSignWrapper != null) [
              "--tpm-ca-sign-wrapper"
              (lib.escapeShellArg cfg.tpmCaSignWrapper)
            ]
            ++ [
              "--audit-log"
              (lib.escapeShellArg cfg.auditLogPath)
            ]
            ++ lib.optionals (cfg.dbPath != null) [
              "--db-path"
              (lib.escapeShellArg cfg.dbPath)
            ]
            ++ lib.optionals (cfg.closureUpstream != null) [
              "--closure-upstream"
              (lib.escapeShellArg cfg.closureUpstream)
            ]
            ++ lib.optionals (cfg.rolloutsDir != null) [
              "--rollouts-dir"
              (lib.escapeShellArg cfg.rolloutsDir)
            ]
            ++ mkPollingSource {
              artifactFlag = "--channel-refs-artifact-url";
              signatureFlag = "--channel-refs-signature-url";
              tokenFlag = "--channel-refs-token-file";
              artifact = cfg.channelRefsSource.artifactUrl;
              signature = cfg.channelRefsSource.signatureUrl;
              tokenFile = cfg.channelRefsSource.tokenFile;
            }
            ++ mkPollingSource {
              artifactFlag = "--revocations-artifact-url";
              signatureFlag = "--revocations-signature-url";
              tokenFlag = "--revocations-token-file";
              artifact = cfg.revocationsSource.artifactUrl;
              signature = cfg.revocationsSource.signatureUrl;
              tokenFile = cfg.revocationsSource.tokenFile;
              tokenFallback = cfg.channelRefsSource.tokenFile;
            }
            ++ mkPollingSource {
              artifactFlag = "--rollouts-source-artifact-url-template";
              signatureFlag = "--rollouts-source-signature-url-template";
              tokenFlag = "--rollouts-source-token-file";
              artifact = cfg.rolloutsSource.artifactUrlTemplate;
              signature = cfg.rolloutsSource.signatureUrlTemplate;
              tokenFile = cfg.rolloutsSource.tokenFile;
              tokenFallback = cfg.channelRefsSource.tokenFile;
            }
          );
          Restart = "always";
          RestartSec = 10;
          StateDirectory = "nixfleet-cp";

          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          # Bundle C: TPM signing needs /dev/tpmrm0 + abrmd dbus. The
          # private-/dev namespace would hide the device. Drop the
          # namespace when TPM is active and harden via DeviceAllow
          # (cgroup BPF) + SupplementaryGroups instead — same posture
          # as the gitea-runner's TPM access in the lab CI flow.
          PrivateDevices = cfg.tpmCaSignWrapper == null;
          DeviceAllow = lib.optionals (cfg.tpmCaSignWrapper != null) [
            "/dev/tpmrm0 rw"
          ];
          SupplementaryGroups = lib.optionals (cfg.tpmCaSignWrapper != null) [
            "tss"
          ];
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          NoNewPrivileges = true;
          ReadWritePaths = ["/var/lib/nixfleet-cp"];
        };
      };

      # GOTCHA: tmpfiles `C` (no `+`) copies only if target absent.
      # The artifact directory is always created — daemon writes there on
      # each successful poll (the channel-refs poll layer no longer needs
      # a bootstrap unit; see #95).
      systemd.tmpfiles.rules = [
        "d /var/lib/nixfleet-cp 0755 root root -"
        "d ${artifactDir} 0755 root root -"
        "C ${cfg.observedPath} 0644 root root - ${initialObservedJson}"
      ];

      networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [listenPort];
    })

    (lib.mkIf cfg.enable {
      nixfleet.persistence.directories = ["/var/lib/nixfleet-cp"];
    })
  ];
}

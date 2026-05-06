{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: {
  options.services.nixfleet-agent = {
    enable = lib.mkEnableOption "NixFleet fleet management agent";

    package = lib.mkOption {
      type = lib.types.package;
      default = inputs.self.packages.${pkgs.system}.nixfleet-agent;
      defaultText = lib.literalExpression "inputs.self.packages.\${pkgs.system}.nixfleet-agent";
      description = ''
        The agent package that provides `bin/nixfleet-agent`. Defaults
        to the flake's crane-built package; tests and pinned-version
        deploys override with their own derivation. Standard NixOS
        `services.<x>.package` escape hatch — accepted as-is, no
        further resolution.
      '';
    };

    controlPlaneUrl = lib.mkOption {
      type = lib.types.str;
      example = "https://fleet.example.com";
      description = "URL of the NixFleet control plane.";
    };

    machineId = lib.mkOption {
      type = lib.types.str;
      default = config.hostSpec.hostName or config.networking.hostName;
      defaultText = lib.literalExpression "config.hostSpec.hostName or config.networking.hostName";
      description = "Machine identifier reported to the control plane.";
    };

    pollInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Poll interval in seconds (steady-state).";
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/agent/trust.json";
      description = ''
        Path to the trust-root JSON file (see docs/trust-root-flow.md §3.4).
        The default is materialised by this module from config.nixfleet.trust
        via environment.etc; override only when sourcing the file from a
        secrets manager.
      '';
    };

    tls = {
      caCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/etc/nixfleet/fleet-ca.pem";
        description = "Path to CA certificate PEM file for verifying the control plane. Trusted alongside system roots.";
      };

      clientCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/agent-cert.pem";
        description = "Path to client certificate PEM file for mTLS authentication.";
      };

      clientKey = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = "/etc/ssh/ssh_host_ed25519_key";
        example = "/etc/ssh/ssh_host_ed25519_key";
        description = ''
          Path to the private key the agent uses to mint CSRs at
          `/v1/enroll` and `/v1/agent/renew`. Defaults to the host's
          SSH ed25519 host key (RFC-0003 §2 binding).

          The CP rejects any CSR whose pubkey doesn't match the host's
          declared `nixfleet.fleetSchema.hosts.<hostname>.pubkey` —
          declare it in `fleet.nix` BEFORE first enrollment. Operators
          previously deploying per-host agent keys via agenix should
          drop those entries (`agents/<host>-key.age` from
          fleet-secrets) once all hosts have rotated to host-key-bound
          certs at their next 30-day renewal cycle.
        '';
      };
    };

    bootstrapTokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/bootstrap-token-host-01";
      description = ''
        Path to a one-shot bootstrap token (operator-minted by
        `nixfleet-mint-token`, signed with the org root key). Used
        by the agent's first-boot enrollment flow only — once the
        cert exists at `tls.clientCert`, the token is never read
        again. Renewal at 50% of cert validity uses the existing
        cert (mTLS-authenticated /v1/agent/renew), not this token.
      '';
    };

    stateDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-agent";
      description = ''
        Directory the agent uses for per-host persistent state.
        Currently holds `last_confirmed_at` — a two-line plaintext
        file binding the agent's most recent successful confirm
        timestamp to the closure it applies to. Pre-created with
        mode 0700 by the platform supervisor (systemd's
        `StateDirectory=` on NixOS; the preActivation script on
        darwin). Survives agent process restart.
      '';
    };

    sshHostKeyFile = lib.mkOption {
      type = lib.types.str;
      default = "/etc/ssh/ssh_host_ed25519_key";
      description = ''
        Host SSH ed25519 private key, used to sign ComplianceFailure
        / RuntimeGateError event payloads. Default matches OpenSSH's
        stock path; override only if the host runs sshd with a
        non-default `HostKey` config.
      '';
    };

    tags = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = ''
        Free-form tags reported with each checkin via the
        `NIXFLEET_TAGS` environment variable. Joined with commas
        before being passed to the agent. Used for operator
        observability (e.g. distinguishing build hosts from
        runners) and ignored by the dispatch decision.
      '';
    };

    complianceGate.mode = lib.mkOption {
      type = lib.types.enum ["auto" "disabled" "permissive" "enforce"];
      default = "auto";
      description = ''
        Local default for the runtime compliance gate.

        - `auto` (default): permissive when the
          compliance-evidence-collector unit is detected on this host
          (systemd `compliance-evidence-collector.service` on NixOS;
          launchd `compliance-evidence-collector` on darwin), disabled
          when absent. Safe for fleets that haven't deployed
          `nixfleet-compliance` — no events posted, no rollouts blocked.
        - `permissive`: the gate runs and posts `RuntimeGateError`
          and `ComplianceFailure` events on failure, but does NOT
          block the activation confirm. Use during incremental
          rollout to observe what would fail without breaking
          deploys.
        - `enforce`: same events posted; additionally a
          `RuntimeGateError` (collector failed / stale evidence)
          triggers a local rollback and skips confirm. Same severity
          class as a SwitchFailed.
        - `disabled`: gate skipped entirely. No events, no journal
          warnings.

        The CP can relay a per-channel
        `EvaluatedTarget.compliance_mode` to override this; when
        absent (or set to `auto`), this value is used.
      '';
    };
  };
}

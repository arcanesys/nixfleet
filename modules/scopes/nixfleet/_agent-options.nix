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
        `services.<x>.package` escape hatch - accepted as-is, no
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

    renewalThresholdFraction = lib.mkOption {
      type = lib.types.nullOr lib.types.float;
      default = null;
      example = 0.5;
      description = ''
        Fraction of cert validity remaining below which the agent
        self-renews. When unset the agent uses its default (0.5,
        renew at half-life). Operators MAY raise this (e.g. 0.8)
        for short-cycle hardware testing of renewal flows.

        Must be strictly between 0 and 1. The agent refuses to
        start if validation fails.
      '';
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/agent/trust.json";
      description = ''
        Path to the trust-root JSON file. The default is materialised
        by this module from config.nixfleet.trust via environment.etc;
        override only when sourcing the file from a secrets manager.
        See docs/rfcs/0005-trust-lifecycle.md §1.5 for the wiring.
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
        default = "/var/lib/nixfleet/agent-cert.pem";
        example = "/var/lib/nixfleet/agent-cert.pem";
        description = ''
          Path to the client certificate PEM file for mTLS
          authentication. Defaults to `/var/lib/nixfleet/agent-cert.pem`
          - a writable, persistent location under the agent's
          stateDir (already in `nixfleet.persistence.directories`).

          Post-RFC-0003-§2 (closed nixfleet#43): the cert is ISSUED
          by `/v1/enroll` and WRITTEN by the agent. It is not
          operator-deployed, so the path must be writable + survive
          reboots. tmpfs paths (e.g. `/run/agenix/...`) break the
          agent's enrollment loop because the bootstrap token is
          one-shot - losing the cert on reboot means the agent can't
          re-enroll on its own.
        '';
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
          declared `nixfleet.fleetSchema.hosts.<hostname>.pubkey`  -
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
        `nixfleet mint-token`, signed with the org root key). Used
        by the agent's first-boot enrollment flow only - once the
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
        Currently holds `last_confirmed_at` - a two-line plaintext
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

    healthChecks = lib.mkOption {
      default = {};
      description = ''
        Operator-declared health probes (issue #86) - load-bearing for
        wave promotion. Each declared probe runs in-agent on its own
        interval; the latest result is reported with every checkin.
        The reconciler gates Healthy -> Soaked promotion on
        `all-probes-passing`, so a host with even one failing probe
        will never advance the wave (mode-dependent - see `mode`
        below).

        Distinct from `complianceGate` (which fronts the external
        `nixfleet-compliance` collector for framework-controls
        evidence): `healthChecks` runs in-process for application-level
        liveness signals declared per host, no external service
        required. The two coexist - a host can have both, and they
        gate at different points in the lifecycle (compliance -> confirm,
        health -> soak).
      '';
      type = lib.types.submodule {
        options = {
          mode = lib.mkOption {
            type = lib.types.enum ["disabled" "permissive" "enforce"];
            default = "enforce";
            description = ''
              - `disabled`: probes don't run; nothing reported. Use
                when the host's NixOS module declares probes but a
                temporary operator override should suppress them.
              - `permissive`: probes run + report, but failures don't
                block soak promotion. Use to observe what would fail
                before flipping to enforce.
              - `enforce` (default): probes run + report + failures
                block soak promotion. The reconciler holds at the
                Healthy -> Soaked transition until all probes pass.
            '';
          };
          http = lib.mkOption {
            type = lib.types.listOf (lib.types.submodule {
              options = {
                name = lib.mkOption {
                  type = lib.types.str;
                  description = "Probe identifier (unique per host).";
                };
                url = lib.mkOption {
                  type = lib.types.str;
                  example = "http://localhost/healthz";
                  description = "Target URL. GET only.";
                };
                expectStatus = lib.mkOption {
                  type = lib.types.port;
                  default = 200;
                  description = "Status code that counts as Pass.";
                };
                intervalSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 30;
                  description = "Run cadence. Lower bound 5s.";
                };
                timeoutSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 5;
                  description = "Per-request timeout; after this Fail.";
                };
              };
            });
            default = [];
            description = ''
              HTTP probes. Each entry runs `GET <url>` on its declared
              `intervalSeconds`; Pass iff response is `expectStatus`.
            '';
          };
          tcp = lib.mkOption {
            type = lib.types.listOf (lib.types.submodule {
              options = {
                name = lib.mkOption {
                  type = lib.types.str;
                  description = "Probe identifier (unique per host).";
                };
                host = lib.mkOption {
                  type = lib.types.str;
                  default = "127.0.0.1";
                  description = "Target host.";
                };
                port = lib.mkOption {
                  type = lib.types.port;
                  description = "Target port; Pass iff connect succeeds.";
                };
                intervalSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 30;
                  description = "Run cadence. Lower bound 5s.";
                };
                timeoutSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 5;
                  description = "Connect timeout; after this Fail.";
                };
              };
            });
            default = [];
            description = ''
              TCP probes. Each entry attempts a connection to
              `host:port` on its declared `intervalSeconds`; Pass iff
              connect succeeds within `timeoutSeconds`.
            '';
          };
          exec = lib.mkOption {
            type = lib.types.listOf (lib.types.submodule {
              options = {
                name = lib.mkOption {
                  type = lib.types.str;
                  description = "Probe identifier (unique per host).";
                };
                command = lib.mkOption {
                  type = lib.types.listOf lib.types.str;
                  example = ["${"$"}{pkgs.curl}/bin/curl" "-fsS" "http://localhost/health"];
                  description = ''
                    Argv. Pass iff exit code is 0 within
                    `timeoutSeconds`. The command runs as the agent's
                    user; declare absolute paths to avoid PATH
                    surprises.
                  '';
                };
                intervalSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 30;
                  description = "Run cadence. Lower bound 5s.";
                };
                timeoutSeconds = lib.mkOption {
                  type = lib.types.int;
                  default = 10;
                  description = "Wallclock timeout for the command; after this Fail.";
                };
              };
            });
            default = [];
            description = ''
              Exec probes. Each entry runs `command` (argv) on its
              declared `intervalSeconds`; Pass iff exit code is 0
              within `timeoutSeconds`.
            '';
          };
        };
      };
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
          `nixfleet-compliance` - no events posted, no rollouts blocked.
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

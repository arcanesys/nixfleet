# Test helper functions for eval and VM checks.
# Usage: import from eval.nix, vm.nix, or vm-nixfleet.nix
#
# `pkgs` is required only because `mkTlsCerts` runs openssl inside a
# `runCommand` derivation. Eval-only callers that never use mkTlsCerts
# still pass pkgs because every callsite already has it in scope (they
# all live under `perSystem = { pkgs, lib, ... }:`).
#
# `inputs` lets VM test node builders reach
# `inputs.nixfleet-scopes.scopes.*` for scope and role modules.
# Eval-only callers can pass `inputs = null` if they never use
# mkTestNode / mkCpNode / mkAgentNode.
{
  lib,
  pkgs,
  inputs ? null,
}: let
  # Core (still nixfleet-local)
  coreNixos = ../../core/_nixos.nix;
  agentModule = ../../scopes/nixfleet/_agent.nix;
  controlPlaneModule = ../../scopes/nixfleet/_control-plane.nix;
  cacheServerModule = ../../scopes/nixfleet/_cache-server.nix;
  cacheModule = ../../scopes/nixfleet/_cache.nix;

  # Common shared-test constants - lives in the `let` scope (not as a
  # top-level attribute of the returned set) so `testPrelude` below
  # can reference it via plain identifier lookup. Re-exported via
  # `inherit` in the returned attrset so callers can still read
  # `helpers.testConstants.apiKey` / `helpers.testConstants.apiKeyHash`.
  testConstants = {
    # Seeded admin API key for CP tests. Plain bearer token for the
    # CLI / curl, and the SHA-256 hash of it for the DB seed via
    # sqlite3 INSERT.
    apiKey = "test-admin-key";
    apiKeyHash = "944650a7cd0f9e14d5c4fb15edbffb7fa45fb9ed36a4fa9be3d7e5476ae51bd9";
  };

  # Superset of every hostname used by any VM test in the tree. Adding
  # a new hostname here is cheap (one extra openssl cert inside the
  # shared derivation) and triggers a single rebuild of `sharedTestCerts`
  # which is then cached for every subsequent run.
  sharedCertHostnames = [
    "web-01"
    "web-02"
    "db-01"
    "agent"
    "operator"
    "builder"
    "cache"
    "tagged"
    "unauth"
    # Single-node subsystem tests in `vm-infra.nix` use `nodes.machine`
    # with `mkAgentNode` for closure dedupe - the shared cert set needs
    # a `machine-cert.pem` entry so mkAgentNode's TLS wiring resolves.
    "machine"
  ];

  # Build a fleet CA + CP server cert + one client cert per hostname.
  # Deterministic and cached - the same `hostnames` list yields the same
  # derivation across tests.
  mkTlsCerts = {hostnames ? ["web-01" "web-02" "db-01"]}:
    pkgs.runCommand "nixfleet-fleet-test-certs" {
      nativeBuildInputs = [pkgs.openssl];
    } ''
      mkdir -p $out

      # Fleet CA (self-signed, EC P-256)
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
        -keyout $out/ca-key.pem -out $out/ca.pem -days 365 -nodes \
        -subj '/CN=nixfleet-test-ca'

      # CP server cert (CN=cp, SAN includes cp + localhost for test curl)
      openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
        -keyout $out/cp-key.pem -out $out/cp-csr.pem -nodes \
        -subj '/CN=cp' \
        -addext 'subjectAltName=DNS:cp,DNS:localhost'
      openssl x509 -req -in $out/cp-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
        -CAcreateserial -out $out/cp-cert.pem -days 365 \
        -copy_extensions copyall

      # Agent client certs (CN = hostname)
      ${lib.concatMapStringsSep "\n" (h: ''
          openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
            -keyout $out/${h}-key.pem -out $out/${h}-csr.pem -nodes \
            -subj "/CN=${h}"
          openssl x509 -req -in $out/${h}-csr.pem -CA $out/ca.pem -CAkey $out/ca-key.pem \
            -CAcreateserial -out $out/${h}-cert.pem -days 365
        '')
        hostnames}

      rm -f $out/*-csr.pem $out/*.srl
    '';

  # The single, shared cert derivation used by every VM scenario. All
  # scenarios receive this as `testCerts` via `scenarioArgs`, which
  # makes their cp + agent node closures dedupe across the fleet.
  sharedTestCerts = mkTlsCerts {hostnames = sharedCertHostnames;};
in {
  inherit testConstants;

  # =====================================================================
  # Eval helpers
  # =====================================================================

  # Build a runCommand that prints PASS/FAIL for each assertion and fails on first failure.
  mkEvalCheck = pkgs: name: assertions:
    pkgs.runCommand "eval-test-${name}" {} (
      lib.concatStringsSep "\n" (
        map (a:
          if a.check
          then ''echo "PASS: ${a.msg}"''
          else ''echo "FAIL: ${a.msg}" >&2; exit 1'')
        assertions
      )
      + "\ntouch $out\n"
    );

  # Default hostSpec values for VM test nodes.
  defaultTestSpec = {
    hostName = "testvm";
    userName = "testuser";
  };

  # =====================================================================
  # TLS / certificate helpers
  # =====================================================================
  #
  # `mkTlsCerts` is parameterised by hostname list; each distinct list
  # produces a distinct runCommand derivation. Historically every VM
  # scenario called it with its own tiny list, which meant:
  #   (a) ~11 distinct cert derivations (one per scenario), each running
  #       openssl 5+ times.
  #   (b) ~11 distinct CP / agent node derivations, because `mkCpNode`
  #       and `mkAgentNode` capture `testCerts` by store path. Two
  #       scenarios with identical CP shapes but different cert derivs
  #       still ended up with different CP system closures.
  #
  # The fix: standardise on ONE shared cert set whose hostname list is
  # a superset of everything any scenario references. Every scenario
  # passes `sharedTestCerts` via `scenarioArgs` and nixosTest dedupes
  # cp + agent closures across scenarios that differ only in testScript.
  #
  # The raw `mkTlsCerts` is still exported for any future test that
  # genuinely needs a different CA shape.
  inherit mkTlsCerts sharedCertHostnames sharedTestCerts;

  # =====================================================================
  # Shared VM scenario test helpers
  # =====================================================================
  # These wrap mkTestNode with the boilerplate every fleet scenario test
  # repeats: TLS cert wiring, server packages, agent TLS flags, etc. All
  # of them take a pre-bound `mkTestNode` (the curried version with
  # inputs/hostSpecModule already applied) and a `defaultTestSpec`, then
  # expose a narrow, scenario-facing API.

  # Wire up /etc/nixfleet-tls/{ca,<certPrefix>-cert,<certPrefix>-key}.pem
  # on a node from a testCerts derivation. Scenarios with "operator" or
  # "builder" style nodes (clients that need the fleet CA + a client
  # cert but don't run agent/CP services) can embed this module directly.
  #
  # Usage: `extraModules = [ (tlsCertsModule { inherit testCerts; certPrefix = "operator"; }) ... ];`
  tlsCertsModule = {
    testCerts,
    certPrefix,
    trustCa ? true,
  }: {
    security.pki.certificateFiles = lib.optional trustCa "${testCerts}/ca.pem";
    environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
    environment.etc."nixfleet-tls/${certPrefix}-cert.pem".source = "${testCerts}/${certPrefix}-cert.pem";
    environment.etc."nixfleet-tls/${certPrefix}-key.pem".source = "${testCerts}/${certPrefix}-key.pem";
  };

  # =====================================================================
  # Python test-script prelude
  # =====================================================================

  # Python preamble injected at the top of every scenario testScript.
  # Provides constants (TEST_KEY / KEY_HASH / AUTH / CURL / API) and
  # a set of helper functions callers reach for repeatedly:
  #
  #   seed_admin_key(node)
  #     Insert the seeded admin API key into the CP's api_keys table.
  #     Every scenario does this once right after `cp.wait_for_unit`.
  #
  #   cp_boot_and_seed(cp)
  #     Boot the CP node, wait for the HTTP port, and seed the admin key.
  #     Fuses 3 lines every scenario writes verbatim into one call.
  #
  #   start_agents(*nodes)
  #     Start each agent node and wait for nixfleet-agent.service to be
  #     active. Handles the common multi-agent case in fleet scenarios.
  #
  #   create_release(cp, entries)
  #     POST /api/v1/releases with the given entries list. Each entry is
  #     a dict of {hostname, store_path} (platform/tags defaulted). Parses
  #     the response and returns the new release id. Panics on non-201.
  #
  #   create_rollout(cp, release_id, tag, **overrides)
  #     POST /api/v1/rollouts with the canonical all-at-once, zero-
  #     tolerance, pause-on-failure shape. `overrides` maps to the JSON
  #     keys of CreateRolloutRequest - a scenario that needs canary or
  #     a threshold passes `strategy="canary"` or `failure_threshold="30%"`.
  #     Returns the new rollout id.
  #
  #   wait_rollout_status(cp, rollout_id, want, timeout=60)
  #     Poll GET /api/v1/rollouts/{id} until `status == want` or the
  #     deadline elapses. Mirrors the Rust harness helper of the same name.
  #
  # `certPrefix` lets agent-side tests use their own client cert instead
  # of the cp-cert.
  #
  # Usage:
  #   testScript = ''
  #     ${helpers.testPrelude { }}
  #
  #     cp_boot_and_seed(cp)
  #     release_id = create_release(cp, [{"hostname": "web-01", "store_path": "/nix/store/x"}])
  #     rollout_id = create_rollout(cp, release_id, "web")
  #     wait_rollout_status(cp, rollout_id, "completed")
  #   '';
  testPrelude = {
    certPrefix ? "cp",
    certDir ? "/etc/nixfleet-tls",
    api ? "https://localhost:8080",
  }: ''
    import json

    TEST_KEY = "${testConstants.apiKey}"
    KEY_HASH = "${testConstants.apiKeyHash}"
    AUTH = f"-H 'Authorization: Bearer {TEST_KEY}'"
    CURL = (
        "curl -sf --cacert ${certDir}/ca.pem "
        "--cert ${certDir}/${certPrefix}-cert.pem "
        "--key ${certDir}/${certPrefix}-key.pem"
    )
    API = "${api}"

    def seed_admin_key(node):
        """Insert the test admin API key into the CP's api_keys table."""
        node.succeed(
            f"sqlite3 /var/lib/nixfleet-cp/state.db "
            f"\"INSERT INTO api_keys (key_hash, name, role) "
            f"VALUES ('{KEY_HASH}', 'test-admin', 'admin')\""
        )

    def cp_boot_and_seed(cp):
        """Start the CP, wait for port 8080, seed the admin API key."""
        cp.start()
        cp.wait_for_unit("nixfleet-control-plane.service")
        cp.wait_for_open_port(8080)
        seed_admin_key(cp)

    def start_agents(*nodes):
        """Start each agent node and wait for nixfleet-agent.service."""
        for node in nodes:
            node.start()
            node.wait_for_unit("nixfleet-agent.service")

    def create_release(cp, entries):
        """POST /api/v1/releases; returns the new release id."""
        body = {
            "flake_ref": "test",
            "flake_rev": "deadbeef",
            "cache_url": None,
            "entries": [
                {
                    "hostname": e["hostname"],
                    "store_path": e["store_path"],
                    "platform": e.get("platform", "x86_64-linux"),
                    "tags": e.get("tags", []),
                }
                for e in entries
            ],
        }
        # Shell-escape single quotes in the payload (close quote,
        # escaped quote, reopen quote). The trailing triple-quote in
        # the source is a nix indented-string escape for a literal
        # pair of single quotes; see note at top of testPrelude.
        payload = json.dumps(body).replace("'", "'\\'''")
        raw = cp.succeed(
            f"{CURL} {AUTH} -X POST "
            f"-H 'Content-Type: application/json' "
            f"-d '{payload}' {API}/api/v1/releases"
        )
        return json.loads(raw)["id"]

    def create_rollout(cp, release_id, tag, **overrides):
        """POST /api/v1/rollouts with the canonical shape; returns rollout id."""
        body = {
            "release_id": release_id,
            "cache_url": None,
            "strategy": "all_at_once",
            "batch_sizes": None,
            "failure_threshold": "0",
            "on_failure": "pause",
            "health_timeout": 60,
            "target": {"tags": [tag]},
            "policy": None,
        }
        body.update(overrides)
        # Shell-escape single quotes in the payload (close quote,
        # escaped quote, reopen quote). The trailing triple-quote in
        # the source is a nix indented-string escape for a literal
        # pair of single quotes; see note at top of testPrelude.
        payload = json.dumps(body).replace("'", "'\\'''")
        raw = cp.succeed(
            f"{CURL} {AUTH} -X POST "
            f"-H 'Content-Type: application/json' "
            f"-d '{payload}' {API}/api/v1/rollouts"
        )
        return json.loads(raw)["rollout_id"]

    def wait_rollout_status(cp, rollout_id, want, timeout=60):
        """Poll GET /api/v1/rollouts/{id} until status == want."""
        import time
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            raw = cp.succeed(f"{CURL} {AUTH} {API}/api/v1/rollouts/{rollout_id}")
            last = json.loads(raw)["status"]
            if last == want:
                return
            time.sleep(1)
        raise Exception(
            f"rollout {rollout_id} did not reach {want} within {timeout}s; "
            f"last status = {last}"
        )
  '';

  # =====================================================================
  # Node builders (mTLS-wired CP + agent + mkTestNode base)
  # =====================================================================

  # Build a CP node with standard mTLS wiring + sqlite + python3.
  #
  # Takes a pre-bound `mkTestNode` (already curried with inputs and
  # hostSpecModule at the aggregator level) and a `testCerts` derivation
  # from mkTlsCerts. Returns a node ready to plug into `nodes.cp`.
  #
  # certPrefix defaults to "cp" so the CP's server cert is read from
  # /etc/nixfleet-tls/cp-cert.pem; override for non-"cp" hostnames.
  mkCpNode = {
    mkTestNode,
    defaultTestSpec,
    testCerts,
    hostName ? "cp",
    certPrefix ? "cp",
    extraModules ? [],
  }:
    mkTestNode {
      hostSpecValues = defaultTestSpec // {inherit hostName;};
      extraModules =
        [
          ({pkgs, ...}: {
            environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
            environment.etc."nixfleet-tls/${certPrefix}-cert.pem".source = "${testCerts}/${certPrefix}-cert.pem";
            environment.etc."nixfleet-tls/${certPrefix}-key.pem".source = "${testCerts}/${certPrefix}-key.pem";

            services.nixfleet-control-plane = {
              enable = true;
              openFirewall = true;
              tls = {
                cert = "/etc/nixfleet-tls/${certPrefix}-cert.pem";
                key = "/etc/nixfleet-tls/${certPrefix}-key.pem";
                clientCa = "/etc/nixfleet-tls/ca.pem";
              };
            };
            environment.systemPackages = [pkgs.sqlite pkgs.python3];
          })
        ]
        ++ extraModules;
    };

  # Build an agent node with standard mTLS wiring + dryRun + tags.
  #
  # Fleet scenario tests need per-host agents with distinct certs and
  # machineIds; this helper bakes in the ceremony. `healthChecks` and
  # `extraModules` give per-scenario customisation without leaking
  # boilerplate into the scenario file.
  mkAgentNode = {
    mkTestNode,
    defaultTestSpec,
    testCerts,
    hostName,
    machineId ? null,
    controlPlaneUrl ? "https://cp:8080",
    tags ? [],
    dryRun ? true,
    pollInterval ? 2,
    healthInterval ? 5,
    healthChecks ? {},
    # Escape hatch for per-scenario nixfleet-agent options that aren't
    # worth a dedicated parameter (e.g. retryInterval, allowInsecure).
    # Merged into services.nixfleet-agent via lib.recursiveUpdate; the
    # default-value keys above still apply, but anything the caller
    # supplies here wins.
    agentExtraConfig ? {},
    extraAgentModules ? [],
    extraModules ? [],
  }: let
    resolvedMachineId =
      if machineId == null
      then hostName
      else machineId;
    baseAgentConfig = {
      enable = true;
      inherit controlPlaneUrl dryRun pollInterval healthInterval tags healthChecks;
      machineId = resolvedMachineId;
      tls = {
        clientCert = "/etc/nixfleet-tls/${hostName}-cert.pem";
        clientKey = "/etc/nixfleet-tls/${hostName}-key.pem";
      };
    };
  in
    mkTestNode {
      hostSpecValues = defaultTestSpec // {inherit hostName;};
      extraModules =
        [
          {
            security.pki.certificateFiles = ["${testCerts}/ca.pem"];

            environment.etc."nixfleet-tls/ca.pem".source = "${testCerts}/ca.pem";
            environment.etc."nixfleet-tls/${hostName}-cert.pem".source = "${testCerts}/${hostName}-cert.pem";
            environment.etc."nixfleet-tls/${hostName}-key.pem".source = "${testCerts}/${hostName}-key.pem";

            services.nixfleet-agent = lib.recursiveUpdate baseAgentConfig agentExtraConfig;
          }
        ]
        ++ extraAgentModules
        ++ extraModules;
    };

  # Build a runNixOSTest-compatible node config with stubbed secrets and known passwords.
  # Returns a NixOS module (attrset) - runNixOSTest handles calling nixosSystem.
  #
  # `inputs` comes from the helpers.nix closure (passed at import time).
  #
  # Parameters:
  #   nixosModules    - additional deferred NixOS modules (e.g. agent/CP)
  #   hmModules       - additional deferred HM modules
  #   hmLinuxModules  - Linux-only HM modules (default [])
  #   hostSpecModule  - path to the hostSpec module
  #   hostSpecValues  - hostSpec attrset for this test node
  #   extraModules    - additional NixOS modules (default [])
  mkTestNode = {
    nixosModules ? [],
    hmModules ? [],
    hmLinuxModules ? [],
    hostSpecModule,
  }: {
    hostSpecValues,
    extraModules ? [],
  }: {
    imports =
      [
        hostSpecModule
        {hostSpec = hostSpecValues;}
        # Framework input modules - test nodes wire these explicitly.
        inputs.disko.nixosModules.disko
        # VM tests compose the workstation role so base/firewall/secrets
        # /HM/backup/impermanence are all declared. Scope options are
        # inert by default (secrets/firewall on by role; impermanence
        # off; backup off; HM on).
        coreNixos
        inputs.nixfleet-scopes.scopes.roles.workstation
        agentModule
        controlPlaneModule
        cacheServerModule
        cacheModule
        {_module.args.inputs = inputs;}
      ]
      ++ nixosModules
      ++ [
        {
          # --- Test user with known password ---
          users.users.${hostSpecValues.userName} = {
            isNormalUser = true;
            group = hostSpecValues.userName;
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };
          users.groups.${hostSpecValues.userName} = {};
          users.users.root = {
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };

          # --- HM config for the test user ---
          home-manager = {
            useGlobalPkgs = true;
            useUserPackages = true;
            users.${hostSpecValues.userName} = {
              imports =
                [hostSpecModule]
                ++ [inputs.nixfleet-scopes.scopes.baseHm inputs.nixfleet-scopes.scopes.impermanenceHm]
                ++ hmModules
                ++ hmLinuxModules;
              hostSpec = hostSpecValues;
              home = {
                stateVersion = "24.11";
                username = hostSpecValues.userName;
                homeDirectory = lib.mkForce "/home/${hostSpecValues.userName}";
                enableNixpkgsReleaseCheck = false;
              };
              systemd.user.startServices = "sd-switch";
            };
          };
        }
      ]
      ++ extraModules;
  };
}

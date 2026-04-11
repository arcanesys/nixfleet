# Test helper functions for eval and VM checks.
# Usage: import from eval.nix, vm.nix, or vm-nixfleet.nix
{lib}: let
  # Import plain scope/core modules (same ones mkHost uses)
  baseScope = import ../../scopes/_base.nix;
  impermanenceScope = import ../../scopes/_impermanence.nix;
  firewallScope = import ../../scopes/_firewall.nix;
  secretsScope = import ../../scopes/_secrets.nix;
  backupScope = import ../../scopes/_backup.nix;
  monitoringScope = import ../../scopes/_monitoring.nix;
  coreNixos = ../../core/_nixos.nix;
  agentModule = ../../scopes/nixfleet/_agent.nix;
  controlPlaneModule = ../../scopes/nixfleet/_control-plane.nix;
  cacheServerModule = ../../scopes/nixfleet/_cache-server.nix;
  cacheModule = ../../scopes/nixfleet/_cache.nix;

  # Common shared-test constants — lives in the `let` scope (not as a
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
in {
  inherit testConstants;

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
    isImpermanent = false;
  };

  # ---------------------------------------------------------------------
  # Shared VM scenario test helpers
  # ---------------------------------------------------------------------
  # These wrap mkTestNode with the boilerplate every fleet scenario test
  # repeats: TLS cert wiring, server packages, agent TLS flags, etc. All
  # of them take a pre-bound `mkTestNode` (the curried version with
  # inputs/hostSpecModule already applied) and a `defaultTestSpec`, then
  # expose a narrow, scenario-facing API.
  #
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

  # Python preamble injected at the top of every scenario testScript.
  # Provides TEST_KEY / KEY_HASH / AUTH constants and a CURL string
  # bound to the given cert files. `certPrefix` lets agent-side tests
  # use their own client cert instead of the cp-cert.
  #
  # Usage:
  #   testScript = ''
  #     ${helpers.testPrelude { }}
  #
  #     cp.start()
  #     ...
  #   '';
  testPrelude = {
    certPrefix ? "cp",
    certDir ? "/etc/nixfleet-tls",
    api ? "https://localhost:8080",
  }: ''
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
  '';

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

  # Build a nixosTest-compatible node config with stubbed secrets and known passwords.
  # Returns a NixOS module (attrset) — nixosTest handles calling nixosSystem.
  #
  # Parameters:
  #   inputs          — flake inputs (needs home-manager, nixpkgs, disko, impermanence)
  #   nixosModules    — additional deferred NixOS modules (e.g. agent/CP)
  #   hmModules       — additional deferred HM modules
  #   hmLinuxModules  — Linux-only HM modules (default [])
  #   hostSpecModule  — path to the hostSpec module
  #   hostSpecValues  — hostSpec attrset for this test node
  #   extraModules    — additional NixOS modules (default [])
  mkTestNode = {
    inputs,
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
        # Framework input modules (disko, impermanence) — mkHost injects these,
        # but test nodes need them explicitly
        inputs.disko.nixosModules.disko
        inputs.impermanence.nixosModules.impermanence
        # Framework core + scopes (plain modules, no longer need `inputs` arg)
        coreNixos
        baseScope.nixos
        impermanenceScope.nixos
        firewallScope.nixos
        secretsScope.nixos
        backupScope.nixos
        monitoringScope.nixos
        agentModule
        controlPlaneModule
        cacheServerModule
        cacheModule
        {_module.args.inputs = inputs;}
      ]
      ++ nixosModules
      ++ [
        inputs.home-manager.nixosModules.home-manager
        {
          # --- Test user with known password ---
          users.users.${hostSpecValues.userName} = {
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };
          users.users.root = {
            hashedPasswordFile = lib.mkForce null;
            password = lib.mkForce "test";
          };

          # --- Handle nixpkgs for test nodes ---
          nixpkgs.pkgs = lib.mkForce (import inputs.nixpkgs {
            system = "x86_64-linux";
            config = {
              allowUnfree = true;
              allowBroken = false;
              allowInsecure = false;
              allowUnsupportedSystem = true;
            };
          });
          nixpkgs.config = lib.mkForce {};
          nixpkgs.hostPlatform = lib.mkForce "x86_64-linux";

          # --- HM config for the test user ---
          home-manager = {
            useGlobalPkgs = true;
            useUserPackages = true;
            users.${hostSpecValues.userName} = {
              imports =
                [hostSpecModule]
                ++ [baseScope.homeManager impermanenceScope.hmLinux]
                ++ hmModules
                ++ hmLinuxModules;
              hostSpec = hostSpecValues;
              home = {
                stateVersion = "24.11";
                username = hostSpecValues.userName;
                homeDirectory = "/home/${hostSpecValues.userName}";
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

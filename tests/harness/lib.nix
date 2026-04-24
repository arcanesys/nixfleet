# tests/harness/lib.nix
#
# Helpers for the microvm.nix-based fleet simulation harness (issue #5).
#
# This module is DIFFERENT from modules/tests/_lib/helpers.nix: that file
# builds full-closure nixosTest nodes for the v0.1 agent/CP. This file
# builds lightweight microVM guests (cloud-hypervisor/qemu) that share the
# host's /nix/store over virtiofs, for cheap fleet-scale scenarios.
#
# Public attrs:
#   mkCpNode       - build a CP microVM NixOS module (serves fleet.resolved.json)
#   mkAgentNode    - build an agent microVM NixOS module (curls CP at boot)
#   mkFleetScenario- wrap one CP + N agents into a runNixOSTest harness
#   mkHarnessCerts - thin wrapper over mkTlsCerts with the harness hostname set
#
# TODO(5): once v0.2 agent/CP skeletons (`crates/agent`, `crates/control-plane`)
# exist, swap the minimal systemd units in nodes/{cp,agent}.nix for the real
# binaries. The node builders should keep the same signature.
{
  lib,
  pkgs,
  inputs,
}: let
  existingHelpers = import ../../modules/tests/_lib/helpers.nix {
    inherit lib pkgs inputs;
  };
  inherit (existingHelpers) mkTlsCerts;

  # One cert set covering the harness hostnames. Additional hostnames get
  # added here as new scenarios land.
  mkHarnessCerts = {hostnames ? ["cp" "agent-01" "agent-02"]}:
    mkTlsCerts {inherit hostnames;};

  # Common microvm guest settings. Cloud-hypervisor is the default because
  # it has the lowest cold-start cost and supports virtiofs /nix/store sharing.
  # mem defaults to 256 MB per guest to fit the 16GB-dev-machine budget
  # (≤512MB per VM allows fleet-20 on a 16GB host).
  microvmGuestDefaults = {
    hypervisor = "qemu";
    mem = 256;
    vcpu = 1;
    # virtiofs share of the host /nix/store keeps cold-start nearly free;
    # the guest mounts it read-only and writes stateful paths elsewhere.
    shares = [
      {
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
        tag = "ro-store";
        proto = "virtiofs";
      }
    ];
    # Bridge-less user-mode networking; every guest sees the host via
    # qemu user-net's 10.0.2.2. Scenarios that need guest-to-guest
    # networking (future: canary rollback) will switch to tap/bridge.
    interfaces = [
      {
        type = "user";
        id = "vm-net";
        mac = "02:00:00:00:00:01";
      }
    ];
  };

  mkCpNode = {
    testCerts,
    resolvedJsonPath,
    hostName ? "cp",
    extraModules ? [],
  }: {
    imports =
      [
        ./nodes/cp.nix
      ]
      ++ extraModules;

    _module.args = {
      inherit testCerts resolvedJsonPath;
      harnessMicrovmDefaults = microvmGuestDefaults;
    };

    networking.hostName = hostName;
    system.stateVersion = lib.mkDefault "24.11";
  };

  mkAgentNode = {
    testCerts,
    hostName,
    controlPlaneHost ? "10.0.2.2",
    controlPlanePort ? 8443,
    extraModules ? [],
  }: {
    imports =
      [
        ./nodes/agent.nix
      ]
      ++ extraModules;

    _module.args = {
      inherit testCerts controlPlaneHost controlPlanePort;
      harnessMicrovmDefaults = microvmGuestDefaults;
      agentHostName = hostName;
    };

    networking.hostName = hostName;
    system.stateVersion = lib.mkDefault "24.11";
  };

  # Wrap one CP + a list of agent modules into a runNixOSTest that boots
  # them as microVMs on a single host VM. The host uses microvm.nixosModules.host
  # to run the guests via systemd. Test script asserts each guest reaches
  # the CP and logs a successful fetch.
  #
  # Extension path: `nodes` is an attrset of name -> { type = "cp"|"agent";
  # module = <nix module>; }. For fleet-N, the scenario file generates
  # agent-01..agent-N programmatically and passes them here.
  mkFleetScenario = {
    name,
    nodes,
    testScript,
    timeout ? 600,
  }:
    pkgs.testers.runNixOSTest {
      inherit name;
      node.specialArgs = {inherit inputs;};

      nodes.host = {pkgs, ...}: {
        imports = [inputs.microvm.nixosModules.host];

        # The host VM needs KVM nested + enough disk for the microvm state
        # dirs + enough RAM to cover every guest's declared mem budget.
        virtualisation = {
          cores = 2;
          memorySize = 4096;
          diskSize = 8192;
          qemu.options = [
            "-cpu"
            "kvm64,+svm,+vmx"
          ];
        };

        microvm.vms = lib.mapAttrs (_: n: {config = n.module;}) nodes;

        environment.systemPackages = [pkgs.jq pkgs.curl];
      };

      inherit testScript;
      meta.timeout = timeout;
    };
in {
  inherit mkCpNode mkAgentNode mkFleetScenario mkHarnessCerts microvmGuestDefaults;
}

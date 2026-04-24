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
#   mkCpHostModule - NixOS module for the host VM that runs the CP stub
#   mkAgentNode    - build an agent microVM NixOS module (curls CP at boot)
#   mkFleetScenario- wrap CP-on-host + N agent microVMs into a runNixOSTest
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

  # CP stub runs on the host VM, not inside a microVM.
  #
  # Rationale: qemu user-mode networking isolates every microVM's
  # gateway (10.0.2.2) to the host VM itself — two user-net microVMs
  # cannot reach each other directly. Running the CP stub on the host
  # VM lets every agent microVM reach it via the shared user-net
  # gateway without bridge/NAT plumbing.
  #
  # TODO(5): when Stream C's v0.2 CP skeleton lands, the same host-VM
  # placement still applies — just swap socat for
  # services.nixfleet-control-plane inside nodes/cp.nix.
  mkCpHostModule = {
    testCerts,
    resolvedJsonPath,
  }: {
    imports = [./nodes/cp.nix];
    _module.args = {inherit testCerts resolvedJsonPath;};
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

  # Wrap a CP host-module + a list of agent microVM modules into a
  # runNixOSTest that boots the host and the agent microVMs. The CP stub
  # runs directly on the host VM (see mkCpHostModule for rationale);
  # agents run as microVMs sharing the host's /nix/store via virtiofs.
  #
  # Extension path: `agents` is an attrset of name -> <nix module>. For
  # fleet-N, the scenario file generates agent-01..agent-N programmatically
  # and passes them here. The CP host module is a single entry.
  mkFleetScenario = {
    name,
    cpHostModule,
    agents,
    testScript,
    timeout ? 600,
  }:
    pkgs.testers.runNixOSTest {
      inherit name;
      node.specialArgs = {inherit inputs;};

      nodes.host = {pkgs, ...}: {
        imports = [
          inputs.microvm.nixosModules.host
          cpHostModule
        ];

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

        microvm.vms = lib.mapAttrs (_: mod: {config = mod;}) agents;

        environment.systemPackages = [pkgs.jq pkgs.curl];
      };

      inherit testScript;
      meta.timeout = timeout;
    };
in {
  inherit mkAgentNode mkCpHostModule mkFleetScenario mkHarnessCerts microvmGuestDefaults;
}

# Firewall hardening — SSH rate limiting, drop logging, nftables.
# Auto-activates on non-minimal hosts.
# Also handles bridge forwarding for microVM hosts.
# Returns { nixos } module attrset. NixOS only (macOS uses pf).
# mkHost imports this directly; it self-activates via lib.mkIf.
{
  nixos = {
    config,
    lib,
    ...
  }: let
    hS = config.hostSpec;
    hasMicrovm = config.services ? nixfleet-microvm-host;
    microvmEnabled = hasMicrovm && config.services.nixfleet-microvm-host.enable;
    bridgeName =
      if hasMicrovm
      then config.services.nixfleet-microvm-host.bridge.name
      else "";
  in {
    config = lib.mkIf (!hS.isMinimal) {
      # Enable nftables backend.
      # Forward-compatible with kernel 6.17+ (which drops ip_tables module).
      # Fleet repos using iptables extraCommands will get an assertion failure —
      # this is intentional, forcing migration before the kernel forces it.
      networking.nftables.enable = true;

      networking.firewall = {
        # Log dropped connections for debugging
        logRefusedConnections = true;
        logReversePathDrops = true;

        # SSH rate limiting — 5 new connections per minute per source IP
        extraInputRules = lib.concatStringsSep "\n" (
          [
            "tcp dport 22 ct state new limit rate 5/minute accept"
            "tcp dport 22 ct state new drop"
          ]
          # Allow DHCP on bridge interface when microVM host is enabled
          ++ lib.optionals microvmEnabled [
            "iifname ${lib.escapeShellArg bridgeName} udp dport 67 accept"
          ]
        );

        # Allow forwarding through the microVM bridge
        extraForwardRules = lib.mkIf microvmEnabled ''
          iifname ${lib.escapeShellArg bridgeName} accept
          oifname ${lib.escapeShellArg bridgeName} ct state established,related accept
        '';
      };
    };
  };
}

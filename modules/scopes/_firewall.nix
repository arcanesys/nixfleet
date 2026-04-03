# Firewall hardening — SSH rate limiting, drop logging, nftables.
# Auto-activates on non-minimal hosts.
# Returns { nixos } module attrset. NixOS only (macOS uses pf).
# mkHost imports this directly; it self-activates via lib.mkIf.
{
  nixos = {
    config,
    lib,
    ...
  }: let
    hS = config.hostSpec;
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
        extraInputRules = ''
          tcp dport 22 ct state new limit rate 5/minute accept
          tcp dport 22 ct state new drop
        '';
      };
    };
  };
}

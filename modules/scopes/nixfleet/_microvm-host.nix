# NixOS module for hosting microVMs as first-class fleet members.
# Provides bridge networking, DHCP, and NAT infrastructure.
# MicroVMs are defined via the upstream microvm.vms option.
# Auto-included by mkHost (disabled by default).
{
  config,
  lib,
  inputs,
  ...
}: let
  cfg = config.services.nixfleet-microvm-host;
  types = lib.types;
in {
  imports = [
    inputs.microvm.nixosModules.host
  ];

  options.services.nixfleet-microvm-host = {
    enable = lib.mkEnableOption "NixFleet MicroVM host";

    bridge = {
      name = lib.mkOption {
        type = types.str;
        default = "nixfleet-br0";
        description = "Bridge interface name for microVM networking.";
      };

      address = lib.mkOption {
        type = types.str;
        default = "10.42.0.1/24";
        description = "Bridge IP address with CIDR prefix.";
      };
    };

    dhcp = {
      enable = lib.mkOption {
        type = types.bool;
        default = true;
        description = "Run dnsmasq DHCP server on the bridge.";
      };

      range = lib.mkOption {
        type = types.str;
        default = "10.42.0.10,10.42.0.254,1h";
        description = "DHCP range in dnsmasq format (start,end,lease-time).";
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      # Bridge interface
      systemd.network = {
        enable = true;
        netdevs."10-${cfg.bridge.name}" = {
          netdevConfig = {
            Kind = "bridge";
            Name = cfg.bridge.name;
          };
        };
        networks."10-${cfg.bridge.name}" = {
          matchConfig.Name = cfg.bridge.name;
          networkConfig = {
            Address = [cfg.bridge.address];
            ConfigureWithoutCarrier = true;
          };
        };
      };

      # IP forwarding for NAT
      boot.kernel.sysctl = {
        "net.ipv4.ip_forward" = 1;
      };

      # NAT for microVM bridge subnet
      networking.nat = {
        enable = true;
        internalInterfaces = [cfg.bridge.name];
      };

      # DHCP server on bridge
      services.dnsmasq = lib.mkIf cfg.dhcp.enable {
        enable = true;
        settings = {
          interface = cfg.bridge.name;
          bind-interfaces = true;
          dhcp-range = [cfg.dhcp.range];
          dhcp-option = [
            "option:router,${lib.head (lib.splitString "/" cfg.bridge.address)}"
          ];
        };
      };
    })

    # Impermanence: persist microVM state. Outer mkIf so
    # environment.persistence isn't referenced on non-impermanent hosts.
    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/microvms"];
    })
  ];
}

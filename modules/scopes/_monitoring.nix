# Monitoring — Prometheus node exporter with fleet-tuned collector defaults.
# Returns { nixos } module attrset.
# mkHost imports this directly; it activates via nixfleet.monitoring.nodeExporter.enable.
{
  nixos = {
    config,
    lib,
    ...
  }: let
    cfg = config.nixfleet.monitoring;
    types = lib.types;
  in {
    options.nixfleet.monitoring = {
      nodeExporter = {
        enable = lib.mkEnableOption "Prometheus node exporter with fleet-tuned defaults";

        port = lib.mkOption {
          type = types.port;
          default = 9100;
          description = "Port for node exporter metrics.";
        };

        openFirewall = lib.mkOption {
          type = types.bool;
          default = false;
          description = "Open node exporter port in the firewall.";
        };

        enabledCollectors = lib.mkOption {
          type = types.listOf types.str;
          default = [
            "systemd"
            "filesystem"
            "cpu"
            "meminfo"
            "netdev"
            "diskstats"
            "loadavg"
            "pressure"
            "time"
          ];
          description = "Node exporter collectors to enable. Fleet repos can override.";
        };

        disabledCollectors = lib.mkOption {
          type = types.listOf types.str;
          default = [
            "textfile"
            "wifi"
            "infiniband"
            "nfs"
            "zfs"
          ];
          description = "Node exporter collectors to disable.";
        };
      };
    };

    config = lib.mkIf cfg.nodeExporter.enable {
      services.prometheus.exporters.node = {
        enable = true;
        port = cfg.nodeExporter.port;
        enabledCollectors = cfg.nodeExporter.enabledCollectors;
        disabledCollectors = cfg.nodeExporter.disabledCollectors;
      };

      networking.firewall.allowedTCPPorts =
        lib.mkIf cfg.nodeExporter.openFirewall [cfg.nodeExporter.port];
    };
  };
}

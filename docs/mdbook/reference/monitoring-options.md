# Monitoring Options

> This module is provided by [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). It is documented here as part of the NixFleet ecosystem reference.

All options under `nixfleet.monitoring.nodeExporter`. The module is auto-included by mkHost and disabled by default. Enable with `nixfleet.monitoring.nodeExporter.enable = true`.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `nodeExporter.enable` | `bool` | `false` | Enable Prometheus node exporter with fleet-tuned defaults. |
| `nodeExporter.port` | `port` | `9100` | Port for node exporter metrics endpoint. |
| `nodeExporter.openFirewall` | `bool` | `false` | Open the node exporter port in the firewall. |
| `nodeExporter.enabledCollectors` | `listOf str` | *(see below)* | Collectors to enable. Fleet repos can override. |
| `nodeExporter.disabledCollectors` | `listOf str` | *(see below)* | Collectors to disable. |

### Default enabled collectors

| Collector | Metrics |
|-----------|---------|
| `systemd` | Systemd unit state and timing |
| `filesystem` | Disk usage per mountpoint |
| `cpu` | CPU utilization |
| `meminfo` | Memory usage |
| `netdev` | Network interface statistics |
| `diskstats` | Disk I/O statistics |
| `loadavg` | System load averages |
| `pressure` | Linux PSI (pressure stall information) |
| `time` | System time and NTP sync status |

### Default disabled collectors

| Collector | Reason |
|-----------|--------|
| `textfile` | Requires external file management - opt-in per host |
| `wifi` | Irrelevant on servers |
| `infiniband` | Not used in typical fleets |
| `nfs` | Not used in typical fleets |
| `zfs` | Framework uses btrfs |

## Systemd service

The scope delegates to NixOS's `services.prometheus.exporters.node` module. The resulting service is `prometheus-node-exporter.service`.

## Example

```nix
nixfleet.monitoring.nodeExporter = {
  enable = true;
  port = 9100;
  openFirewall = true;  # allow Prometheus scrape from monitoring host
};
```

To add a collector not in the default set:

```nix
nixfleet.monitoring.nodeExporter.enabledCollectors =
  config.nixfleet.monitoring.nodeExporter.enabledCollectors ++ ["textfile"];
```

Fleet repos that use a Prometheus stack typically scrape all hosts on port 9100. Pair with a firewall rule on the monitoring host to restrict access to the scrape network.

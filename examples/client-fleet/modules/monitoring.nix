# Example: Node exporter on all hosts + Prometheus server on monitoring host.
# Prerequisites:
#   - One host designated as monitoring server
#   - Fleet repos know their host inventory for scrape targets
#
# Usage: add the nodeExporter block to all hosts, Prometheus server to one host.
{...}: {
  # Enable node exporter with framework defaults on all hosts
  nixfleet.monitoring.nodeExporter = {
    enable = true;
    openFirewall = true;
  };

  # Example: Prometheus server (enable on your monitoring host only)
  # Uncomment and customize:
  #
  # services.prometheus = {
  #   enable = true;
  #   scrapeConfigs = [
  #     {
  #       job_name = "fleet-nodes";
  #       static_configs = [{
  #         targets = [
  #           "web-01:9100"
  #           "web-02:9100"
  #           "srv-01:9100"
  #         ];
  #       }];
  #     }
  #     {
  #       job_name = "nixfleet-cp";
  #       static_configs = [{
  #         targets = ["srv-01:8080"];
  #       }];
  #     }
  #   ];
  # };
}

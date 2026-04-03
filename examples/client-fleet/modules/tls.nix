# Example: mTLS setup for agent ↔ control plane communication.
# Prerequisites:
#   - A fleet CA (e.g., generated with step-ca, cfssl, or openssl)
#   - CP server cert + key signed by the fleet CA
#   - Per-agent client certs signed by the fleet CA
#   - All certs stored as agenix/sops secrets
#
# Usage: add this module to your host's `modules` list in fleet.nix.
# Split into CP-specific and agent-specific modules for real fleets.
{
  config,
  lib,
  ...
}: {
  # --- Control Plane host ---
  # Enable only on the host running the control plane.
  services.nixfleet-control-plane = lib.mkIf (config.networking.hostName == "cp-host") {
    tls = {
      cert = config.age.secrets.cp-cert.path;
      key = config.age.secrets.cp-key.path;
      clientCa = config.age.secrets.fleet-ca.path; # enables mTLS
    };
  };

  # --- Agent (all managed hosts) ---
  # Every agent authenticates to the CP with a client cert.
  services.nixfleet-agent = lib.mkIf config.services.nixfleet-agent.enable {
    controlPlaneUrl = "https://cp-host.fleet.internal:8080";
    tls = {
      clientCert = config.age.secrets.agent-cert.path;
      clientKey = config.age.secrets.agent-key.path;
    };
  };
}

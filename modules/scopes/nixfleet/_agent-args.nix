{
  lib,
  cfg,
  package,
}:
[
  "${package}/bin/nixfleet-agent"
  "--control-plane-url"
  (lib.escapeShellArg cfg.controlPlaneUrl)
  "--machine-id"
  (lib.escapeShellArg cfg.machineId)
  "--poll-interval"
  (toString cfg.pollInterval)
  "--trust-file"
  (lib.escapeShellArg (toString cfg.trustFile))
]
++ lib.optionals (cfg.renewalThresholdFraction != null) [
  "--renewal-threshold-fraction"
  (toString cfg.renewalThresholdFraction)
]
++ lib.optionals (cfg.tls.caCert != null) [
  "--ca-cert"
  (lib.escapeShellArg cfg.tls.caCert)
]
++ lib.optionals (cfg.tls.clientCert != null) [
  "--client-cert"
  (lib.escapeShellArg cfg.tls.clientCert)
]
++ lib.optionals (cfg.tls.clientKey != null) [
  "--client-key"
  (lib.escapeShellArg cfg.tls.clientKey)
]
++ lib.optionals (cfg.bootstrapTokenFile != null) [
  "--bootstrap-token-file"
  (lib.escapeShellArg cfg.bootstrapTokenFile)
]
++ [
  "--state-dir"
  (lib.escapeShellArg cfg.stateDir)
  "--compliance-gate-mode"
  (lib.escapeShellArg cfg.complianceGate.mode)
  "--ssh-host-key-file"
  (lib.escapeShellArg cfg.sshHostKeyFile)
]
# Issue #86: only pass --health-checks-config when probes are declared.
# Empty/absent -> agent runs without a probe scheduler (no checkin field
# overhead, no /etc file written).
++ lib.optionals (
  cfg.healthChecks.http
  != []
  || cfg.healthChecks.tcp != []
  || cfg.healthChecks.exec != []
) [
  "--health-checks-config"
  (lib.escapeShellArg "/etc/nixfleet/agent/health-checks.json")
]

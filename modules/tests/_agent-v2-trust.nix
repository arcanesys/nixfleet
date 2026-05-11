# FOOTGUN: eval-only - don't builtins.readFile the trust.json derivation (IFD).
{
  lib,
  cfg,
  ...
}: let
  execStart = cfg.systemd.services.nixfleet-agent.serviceConfig.ExecStart;
  trustEtc = cfg.environment.etc."nixfleet/agent/trust.json";
  trust = cfg.nixfleet.trust;
in [
  {
    check = trustEtc ? source;
    msg = "agent scope materialises environment.etc.\"nixfleet/agent/trust.json\".source";
  }
  {
    check = lib.hasInfix "trust.json" (baseNameOf trustEtc.source);
    msg = "agent trust.json store path name matches pkgs.writers.writeJSON output";
  }
  {
    check = trust.ciReleaseKey.current != null;
    msg = "agent fixture has ciReleaseKey.current set (non-null for meaningful trust.json)";
  }
  {
    check = lib.hasInfix "--trust-file" execStart;
    msg = "agent ExecStart carries --trust-file flag";
  }
  {
    check = lib.hasInfix "--control-plane-url" execStart;
    msg = "agent ExecStart carries --control-plane-url flag";
  }
  {
    check = lib.hasInfix "--machine-id" execStart;
    msg = "agent ExecStart carries --machine-id flag";
  }
  {
    check = lib.hasInfix "--poll-interval" execStart;
    msg = "agent ExecStart carries --poll-interval flag";
  }
  {
    check = lib.hasInfix "/etc/nixfleet/agent/trust.json" execStart;
    msg = "agent ExecStart passes the canonical trust-file path";
  }
]

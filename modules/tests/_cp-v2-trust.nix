{
  lib,
  cfg,
  ...
}: let
  execStart = cfg.systemd.services.nixfleet-control-plane.serviceConfig.ExecStart;
  trustEtc = cfg.environment.etc."nixfleet/cp/trust.json";
  svcCfg = cfg.services.nixfleet-control-plane;
  trust = cfg.nixfleet.trust;
  timer = cfg.systemd.timers.nixfleet-control-plane;
in [
  {
    check = trustEtc ? source;
    msg = "CP scope materialises environment.etc.\"nixfleet/cp/trust.json\".source";
  }
  {
    check = lib.hasInfix "trust.json" (baseNameOf trustEtc.source);
    msg = "CP trust.json store path name matches pkgs.writers.writeJSON output";
  }
  {
    check = trust.ciReleaseKey.current != null;
    msg = "CP fixture has ciReleaseKey.current set (non-null for meaningful trust.json)";
  }
  {
    check = lib.hasInfix "--trust-file" execStart;
    msg = "CP ExecStart carries --trust-file flag";
  }
  {
    check = lib.hasInfix "--artifact" execStart;
    msg = "CP ExecStart carries --artifact flag";
  }
  {
    check = lib.hasInfix "--signature" execStart;
    msg = "CP ExecStart carries --signature flag";
  }
  {
    check = lib.hasInfix "--observed" execStart;
    msg = "CP ExecStart carries --observed flag";
  }
  {
    check = lib.hasInfix "--freshness-window-secs" execStart;
    msg = "CP ExecStart carries --freshness-window-secs flag";
  }
  {
    check = lib.hasInfix "/etc/nixfleet/cp/trust.json" execStart;
    msg = "CP ExecStart passes the canonical trust-file path";
  }
  {
    check = svcCfg.artifactPath == "/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json";
    msg = "CP artifactPath defaults under /var/lib/nixfleet-cp/fleet/releases/";
  }
  {
    check = svcCfg.signaturePath == "/var/lib/nixfleet-cp/fleet/releases/fleet.resolved.json.sig";
    msg = "CP signaturePath defaults pair with artifactPath";
  }
  {
    check = svcCfg.observedPath == "/var/lib/nixfleet-cp/observed.json";
    msg = "CP observedPath defaults under /var/lib/nixfleet-cp/";
  }
  {
    check = toString svcCfg.trustFile == "/etc/nixfleet/cp/trust.json";
    msg = "CP trustFile defaults to /etc/nixfleet/cp/trust.json";
  }
  {
    check = cfg.systemd.services.nixfleet-control-plane.serviceConfig.Type == "oneshot";
    msg = "CP service is oneshot (timer-driven, not long-running)";
  }
  {
    check = timer.wantedBy == ["timers.target"];
    msg = "CP timer is wantedBy timers.target";
  }
  {
    check = lib.hasInfix "m" timer.timerConfig.OnUnitActiveSec;
    msg = "CP timer OnUnitActiveSec is in minutes";
  }
]

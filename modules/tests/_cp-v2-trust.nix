# modules/tests/_cp-v2-trust.nix
#
# Eval-only assertions for the v0.2 control-plane scope module
# (modules/scopes/nixfleet/_control-plane.nix). Mirrors
# _agent-v2-trust.nix and additionally verifies:
#   - --trust-file, --db-path, --release-path land on the ExecStart.
#   - Default paths match the contract (trust.json under /etc/nixfleet/cp,
#     release artifact under /var/lib/nixfleet-cp/fleet.git/releases).
#
# Called from modules/tests/eval.nix. Imported (not auto-imported by
# import-tree) because the filename starts with an underscore.
{
  lib,
  cfg,
  ...
}: let
  execStart = cfg.systemd.services.nixfleet-control-plane.serviceConfig.ExecStart;
  trustEtc = cfg.environment.etc."nixfleet/cp/trust.json";
  svcCfg = cfg.services.nixfleet-control-plane;
  trust = cfg.nixfleet.trust;
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
    check = lib.hasInfix "--db-path" execStart;
    msg = "CP ExecStart carries --db-path flag";
  }
  {
    check = lib.hasInfix "--release-path" execStart;
    msg = "CP ExecStart carries --release-path flag";
  }
  {
    check = lib.hasInfix "/etc/nixfleet/cp/trust.json" execStart;
    msg = "CP ExecStart passes the canonical trust-file path";
  }
  {
    check = toString svcCfg.releasePath == "/var/lib/nixfleet-cp/fleet.git/releases/fleet.resolved.json";
    msg = "CP releasePath defaults per trust-root-flow.md §4 option (b)";
  }
  {
    check = toString svcCfg.trustFile == "/etc/nixfleet/cp/trust.json";
    msg = "CP trustFile defaults to /etc/nixfleet/cp/trust.json";
  }
  {
    check = svcCfg.dbPath == "/var/lib/nixfleet-cp/state.db";
    msg = "CP dbPath defaults to /var/lib/nixfleet-cp/state.db";
  }
]

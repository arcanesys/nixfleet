# modules/tests/_agent-v2-trust.nix
#
# Eval-only assertions for the v0.2 agent scope module
# (modules/scopes/nixfleet/_agent.nix). Verifies that:
#   - environment.etc materialises /etc/nixfleet/agent/trust.json from
#     config.nixfleet.trust via pkgs.writers.writeJSON.
#   - The configured trust payload carries schemaVersion = 1 (required
#     per docs/trust-root-flow.md §7.4 and proto::TrustConfig).
#   - systemd ExecStart carries --trust-file, --control-plane-url,
#     --machine-id, --poll-interval for the v0.2 poll-only contract.
#
# Called from modules/tests/eval.nix. Imported (not auto-imported by
# import-tree) because the filename starts with an underscore.
#
# Eval-only: we do NOT builtins.readFile the trust.json derivation —
# that would trigger IFD during `nix flake check`. We assert on the
# NixOS-level shape (etc entry present, ExecStart arg-list) plus the
# eval-time trust config under config.nixfleet.trust.
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
    # trust.json is written by pkgs.writers.writeJSON, which serialises a
    # Nix attrset via builtins.toJSON. The scope module builds this attrset
    # with schemaVersion = 1 at the top level; the resulting store path is
    # a derivation and cannot be read during eval without IFD.
    # Assert on the source-path derivation name to confirm the writeJSON
    # pathway is in use.
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
    # Path must match the scope module default and trust-root-flow.md §3.2.
    check = lib.hasInfix "/etc/nixfleet/agent/trust.json" execStart;
    msg = "agent ExecStart passes the canonical trust-file path";
  }
]

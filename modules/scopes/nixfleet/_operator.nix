{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.nixfleet.operator;
  nixfleet-cli = inputs.self.packages.${pkgs.system}.nixfleet-cli;
in {
  options.nixfleet.operator = {
    enable = lib.mkEnableOption ''
      operator-workstation tooling: installs the `nixfleet` umbrella
      binary (with subcommands `status`, `rollout trace`, `config init`,
      `mint-token`, `derive-pubkey`, `mint-operator-cert`) system-wide.
    '';

    orgRootKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/org-root-key";
      description = ''
        Path to the org root ed25519 private key (raw 32 bytes),
        decrypted by the fleet's secrets backend. Used by
        `nixfleet mint-token --org-root-key` when the operator runs
        the subcommand interactively. The path is not consumed by any
        systemd service; it's only read when the operator invokes
        the subcommand.

        Set on the operator's workstation only - `null` on every
        other host.
      '';
    };

    fleetRootCertFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/home/operator/.config/nixfleet/fleet-root.cert.pem";
      description = ''
        Path to the offline fleet root CA cert PEM. Read by
        `nixfleet mint-operator-cert` to issue per-workstation
        operator certs. Public material; safe to live in the
        operator's home with mode 0644.

        Set on the operator's workstation only - `null` elsewhere.
      '';
    };

    fleetRootKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/home/operator/.config/nixfleet/fleet-root.key.pem";
      description = ''
        Path to the offline fleet root CA private key PEM. Read by
        `nixfleet mint-operator-cert` to issue per-workstation
        operator certs. Never read by any systemd service; only the
        operator-invoked subcommand touches this path.

        Set on the operator's workstation only - `null` elsewhere.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [nixfleet-cli];

    environment.variables = lib.filterAttrs (_: v: v != null) {
      NIXFLEET_OPERATOR_ORG_ROOT_KEY = cfg.orgRootKeyFile;
      NIXFLEET_OPERATOR_FLEET_ROOT_CERT_FILE = cfg.fleetRootCertFile;
      NIXFLEET_OPERATOR_FLEET_ROOT_KEY_FILE = cfg.fleetRootKeyFile;
    };
  };
}

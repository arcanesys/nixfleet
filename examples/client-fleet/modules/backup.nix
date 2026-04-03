# Example: Restic backup plugged into framework's backup harness.
# Prerequisites:
#   - A restic repository (S3, local, SFTP, etc.)
#   - Credentials stored as an agenix/sops secret (RESTIC_REPOSITORY, RESTIC_PASSWORD, AWS_*)
#
# Usage: add this module to your host's `modules` list in fleet.nix.
{
  config,
  pkgs,
  lib,
  ...
}: let
  cfg = config.nixfleet.backup;
  excludeFile =
    pkgs.writeText "restic-excludes"
    (lib.concatStringsSep "\n" cfg.exclude);
in {
  nixfleet.backup = {
    enable = true;
    paths = ["/persist"];
    schedule = "*-*-* 02:00:00";
    healthCheck.onSuccess = "https://hc-ping.com/your-uuid-here";
  };

  # Wire the actual backup command
  systemd.services.nixfleet-backup.serviceConfig.ExecStart = lib.concatStringsSep " " [
    "${pkgs.restic}/bin/restic"
    "backup"
    (lib.concatStringsSep " " cfg.paths)
    "--exclude-file=${excludeFile}"
  ];

  # Retention pruning as post-hook
  nixfleet.backup.postHook = ''
    ${pkgs.restic}/bin/restic forget \
      --keep-daily ${toString cfg.retention.daily} \
      --keep-weekly ${toString cfg.retention.weekly} \
      --keep-monthly ${toString cfg.retention.monthly} \
      --prune
  '';

  # Credentials from encrypted secrets
  systemd.services.nixfleet-backup.serviceConfig.EnvironmentFile =
    config.age.secrets.restic-env.path;
}

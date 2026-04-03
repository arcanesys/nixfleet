# Backup Options

All options under `nixfleet.backup`. The module is auto-included by mkHost and disabled by default. Enable with `nixfleet.backup.enable = true`.

The backup scope is backend-agnostic. It creates the systemd timer and service skeleton. Fleet repos set `systemd.services.nixfleet-backup.serviceConfig.ExecStart` to their chosen backup tool (restic, borgbackup, etc.).

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable NixFleet backup scaffolding (timer, health, persistence). |
| `paths` | `listOf str` | `["/persist"]` | Directories to back up. |
| `exclude` | `listOf str` | `["/persist/nix" "*.cache"]` | Patterns to exclude from backup. |
| `schedule` | `str` | `"daily"` | Systemd calendar expression (e.g., `daily`, `weekly`, `*-*-* 02:00:00`). |
| `retention.daily` | `int` | `7` | Number of daily snapshots to keep. |
| `retention.weekly` | `int` | `4` | Number of weekly snapshots to keep. |
| `retention.monthly` | `int` | `6` | Number of monthly snapshots to keep. |
| `healthCheck.onSuccess` | `nullOr str` | `null` | URL to GET on successful backup (e.g., Healthchecks.io ping URL). |
| `healthCheck.onFailure` | `nullOr str` | `null` | URL to GET on backup failure. |
| `preHook` | `lines` | `""` | Shell commands to run before backup. |
| `postHook` | `lines` | `""` | Shell commands to run after successful backup. |
| `stateDirectory` | `str` | `"/var/lib/nixfleet-backup"` | Directory for backup state/cache. Persisted on impermanent hosts. |

## Systemd timer

| Setting | Value |
|---------|-------|
| Unit | `nixfleet-backup.timer` |
| WantedBy | `timers.target` |
| OnCalendar | value of `schedule` |
| Persistent | `true` (catch up on missed runs) |
| RandomizedDelaySec | `1h` (stagger across fleet) |

## Systemd service

| Setting | Value |
|---------|-------|
| Unit | `nixfleet-backup.service` |
| Type | `oneshot` |
| After | `network-online.target` |
| Wants | `network-online.target` |
| StateDirectory | `nixfleet-backup` |
| ExecStart | *(set by fleet module)* |

After a successful backup run, the service writes `status.json` to `stateDirectory`:

```json
{"lastRun": "2025-01-15T02:00:00+00:00", "status": "success", "hostname": "web-01"}
```

When `healthCheck.onFailure` is set, a companion `nixfleet-backup-failure.service` is registered as the `OnFailure` handler.

## Impermanence

On impermanent hosts (`hostSpec.isImpermanent = true`), the scope automatically persists `stateDirectory`.

## Example — restic

```nix
{config, ...}: {
  nixfleet.backup = {
    enable = true;
    paths = ["/persist/home" "/persist/var/lib"];
    schedule = "*-*-* 03:00:00";
    retention = { daily = 7; weekly = 4; monthly = 3; };
    healthCheck.onSuccess = "https://hc-ping.com/your-uuid-here";
  };

  # Wire in restic as the backend
  systemd.services.nixfleet-backup.serviceConfig.ExecStart = let
    resticCmd = "${pkgs.restic}/bin/restic";
    repo = "s3:s3.amazonaws.com/my-bucket/backups";
  in ''
    ${resticCmd} -r ${repo} backup \
      ${builtins.concatStringsSep " " config.nixfleet.backup.paths} \
      ${builtins.concatStringsSep " " (map (p: "--exclude=${p}") config.nixfleet.backup.exclude)} \
      --forget \
      --keep-daily ${toString config.nixfleet.backup.retention.daily} \
      --keep-weekly ${toString config.nixfleet.backup.retention.weekly} \
      --keep-monthly ${toString config.nixfleet.backup.retention.monthly}
  '';

  # restic repository password
  age.secrets.restic-password.file = "${inputs.secrets}/restic-password.age";
  systemd.services.nixfleet-backup.serviceConfig.EnvironmentFile =
    config.age.secrets.restic-password.path;
}
```

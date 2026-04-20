# Backup Options

> This module is provided by [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). It is documented here as part of the NixFleet ecosystem reference.

All options under `nixfleet.backup`. The module is auto-included by mkHost and disabled by default. Enable with `nixfleet.backup.enable = true`.

The backup scope is backend-agnostic. It creates the systemd timer and service skeleton. Set `backend` to `"restic"` or `"borgbackup"` to use a built-in backend, or set `systemd.services.nixfleet-backup.serviceConfig.ExecStart` directly to use any other tool.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable NixFleet backup scaffolding (timer, health, persistence). |
| `backend` | `nullOr enum ["restic" "borgbackup"]` | `null` | Backup backend. Null = fleet sets ExecStart manually. |
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

## restic backend options

Active when `backend = "restic"`. The `restic` package is added to `environment.systemPackages` automatically.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `restic.repository` | `str` | `""` | Restic repository URL or path. Example: `"/mnt/backup/restic"`. |
| `restic.passwordFile` | `str` | `""` | Path to file containing the repository password. Example: `"/run/secrets/restic-password"`. |

## borgbackup backend options

Active when `backend = "borgbackup"`. The `borgbackup` package is added to `environment.systemPackages` automatically.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `borgbackup.repository` | `str` | `""` | Borg repository path or `ssh://user@host/path`. |
| `borgbackup.passphraseFile` | `nullOr str` | `null` | Path to file containing the repository passphrase. Null = repokey without passphrase. |
| `borgbackup.encryption` | `str` | `"repokey"` | Borg encryption mode (`repokey`, `repokey-blake2`, `none`, etc.). |

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

On impermanent hosts (`nixfleet.impermanence.enable = true`), the scope automatically persists `stateDirectory`.

## Example - restic (built-in backend)

```nix
nixfleet.backup = {
  enable = true;
  backend = "restic";
  paths = ["/persist/home" "/persist/var/lib"];
  schedule = "*-*-* 03:00:00";
  retention = { daily = 7; weekly = 4; monthly = 3; };
  healthCheck.onSuccess = "https://hc-ping.com/your-uuid-here";
  restic = {
    repository = "s3:s3.amazonaws.com/my-bucket/backups";
    passwordFile = "/run/secrets/restic-password";
  };
};
```

## Example - borgbackup (built-in backend)

```nix
nixfleet.backup = {
  enable = true;
  backend = "borgbackup";
  paths = ["/persist/home" "/persist/var/lib"];
  schedule = "weekly";
  retention = { daily = 7; weekly = 4; monthly = 6; };
  borgbackup = {
    repository = "ssh://backup-user@backup-host/var/backups/myhost";
    passphraseFile = "/run/secrets/borg-passphrase";
    encryption = "repokey-blake2";
  };
};
```

## Example - custom backend (manual ExecStart)

```nix
{config, pkgs, ...}: {
  nixfleet.backup = {
    enable = true;
    paths = ["/persist/home" "/persist/var/lib"];
    schedule = "*-*-* 03:00:00";
    retention = { daily = 7; weekly = 4; monthly = 3; };
    healthCheck.onSuccess = "https://hc-ping.com/your-uuid-here";
  };

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
}
```

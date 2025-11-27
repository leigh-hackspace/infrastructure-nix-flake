{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ../config.nix;
  slackWebhookUrl = lib.strings.trim (builtins.readFile CONFIG.SLACK_URL_FILE);
in
{
  systemd.services.backup-srv = {
    description = "Backup /srv directory";
    serviceConfig = {
      Type = "oneshot";
      ExecStartPost = "${pkgs.curl}/bin/curl -X POST -H 'Content-type: application/json' --data $data '{\"text\":\"Container volumes backup complete\"}' ${slackWebhookUrl}";
    };
    path = with pkgs; [
      gnutar
      gzip
      coreutils
    ];
    script = ''
      BACKUP_DIR="/mnt/backups/services1.int.leighhack.org/srv"
      TIMESTAMP=$(date +%Y%m%d_%H%M%S)
      BACKUP_FILE="$BACKUP_DIR/srv-backup-$TIMESTAMP.tar.gz"

      # Create backup directory if it doesn't exist
      mkdir -p "$BACKUP_DIR"

      # Create the backup
      tar -czf "$BACKUP_FILE" --ignore-failed-read -C / srv

      # Ensure correct permissions
      chown backups:backups $BACKUP_FILE

      # Keep only the last 7 backups
      cd "$BACKUP_DIR"
      ls -t srv-backup-*.tar.gz | tail -n +8 | xargs -r rm

      echo "Backup completed: $BACKUP_FILE"
    '';
  };

  systemd.timers.backup-srv = {
    description = "Daily backup of /srv directory";
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnCalendar = "daily";
      Persistent = true;
      RandomizedDelaySec = "1h";
    };
  };
}

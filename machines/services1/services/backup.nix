{
  lib,
  pkgs,
  ...
}:

let
  CONFIG = import ../config.nix;
  slackWebhookUrl = lib.strings.trim (builtins.readFile CONFIG.SLACK_URL_FILE);
in
{
  # # List all backups
  # sudo list-backups-srv
  #
  # # Restore a backup to the current dir
  # sudo restore-backup-srv services1-backup-srv-2025-12-11T10:09:54
  environment.systemPackages = [
    (pkgs.writeShellScriptBin "list-backups-srv" ''
      export BORG_RSH="ssh -i ${CONFIG.BACKUP_KEY_FILE}"
      borg list ssh://backups@nas2.int.leighhack.org:3022/backups/services1.int.leighhack.org/borg/srv
    '')

    (pkgs.writeShellScriptBin "restore-backup-srv" ''
      export BORG_RSH="ssh -i ${CONFIG.BACKUP_KEY_FILE}"
      borg extract --list ssh://backups@nas2.int.leighhack.org:3022/backups/services1.int.leighhack.org/borg/srv::$1 /srv
    '')
  ];

  services.borgbackup.jobs.backup-srv = {
    paths = "/srv";
    encryption.mode = "none";
    environment.BORG_RSH = "ssh -i ${CONFIG.BACKUP_KEY_FILE}";
    repo = "ssh://backups@nas2.int.leighhack.org:3022/backups/services1.int.leighhack.org/borg/srv";
    compression = "auto,zstd";
    startAt = "daily";
    postHook = ''
      if [ $exitStatus -eq 0 ]; then
        ${pkgs.curl}/bin/curl -X POST -H 'Content-type: application/json' --data '{"text":"Container volumes backup complete"}' ${slackWebhookUrl}
      fi
    '';
  };
}

{
  lib,
  pkgs,
  config,
  ...
}:

let
  CONFIG = import ../config.nix;
  slackWebhookUrl = lib.strings.trim (builtins.readFile (config.sopsSecretText "slack_url"));
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

  # journalctl -u borgbackup-job-backup-srv -b
  services.borgbackup.jobs.backup-srv = {
    paths = "/srv";
    # Deliberately unencrypted: the repo lives on the NAS share that is already
    # the trust boundary (access-controlled `backups` user over ssh with
    # CONFIG.BACKUP_KEY_FILE, on the LAN), and the payloads here are container
    # volumes whose secrets live elsewhere (CONFIG.ENV_FILE, Postgres).  If the
    # NAS ever stops being trusted — off-site copies, a share exposed beyond the
    # LAN — switch this to repokey-blake2 and keep the key off the NAS.
    #
    # Caveat before relying on "access-controlled": the NAS ssh host key is not
    # pinned for this job (BORG_RSH below has no StrictHostKeyChecking/
    # UserKnownHostsFile, unlike the dns-sync and network-status ssh calls), and
    # the job runs as root, so the real boundary is the NAS plus root on
    # services1.
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

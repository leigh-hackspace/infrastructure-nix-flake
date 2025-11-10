{
  config,
  lib,
  pkgs,
  ...
}:

let
  CONFIG = import ../config.nix;
in
{
  # sudo systemctl start gocardless-authentik-sync
  # journalctl -u gocardless-authentik-sync -f
  systemd.services.gocardless-authentik-sync = {
    description = "Gocardless Authentik Sync";
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.gocardless-tools}/bin/gocardless-tools --gocardless-token $GOCARDLESS_TOKEN --authentik-token $AUTHENTIK_TOKEN";
      EnvironmentFile = CONFIG.ENV_FILE;
    };
  };

  systemd.timers.gocardless-authentik-sync = {
    timerConfig = {
      Unit = "gocardless-authentik-sync.service";
      OnCalendar = "*-*-* 01:00:00";
    };
    wantedBy = [ "timers.target" ];
  };
}

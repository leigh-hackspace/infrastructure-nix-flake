{
  config,
  lib,
  pkgs,
  ...
}:

{
  systemd.services.gocardless-authentik-sync = {
    description = "Gocardless Authentik Sync";
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.gocardless-tools}/bin/main --gocardless-token $GOCARDLESS_TOKEN --authentik-token $AUTHENTIK_TOKEN";
      EnvironmentFile = "/home/leigh-admin/Projects/infrastructure-nix-flake/secrets/gocardless-authentik-sync-env";
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

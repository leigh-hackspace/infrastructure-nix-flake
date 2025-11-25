{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ./config.nix;
  slackWebhookUrl = lib.strings.trim (builtins.readFile CONFIG.SLACK_URL_FILE);
  update-containers = (
    pkgs.writers.writeNuBin "update-containers" (
      pkgs.replaceVars ../../nu/update-containers.nu {
        PODMAN = lib.getExe pkgs.podman;
        SYSTEMCTL = "${pkgs.systemd}/bin/systemctl";
        CURL = lib.getExe pkgs.curl;
        SLACK_URL = slackWebhookUrl;
      }
    )
  );
in
{
  environment.systemPackages = [
    update-containers
  ];

  systemd.timers = {
    update-containers = {
      timerConfig = {
        Unit = "update-containers.service";
        OnCalendar = "*-*-* 02:00:00"; # Run everyday at 2am
        Persistent = true; # Run missed timers on boot
      };
      wantedBy = [ "timers.target" ];
    };
  };

  # sudo systemctl start update-containers.service
  # journalctl -u update-containers.service -f
  systemd.services = {
    update-containers = {
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${update-containers}/bin/update-containers";
      };
      # Add these for better logging
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
    };
  };

  virtualisation.podman = {
    enable = true;
    autoPrune.enable = true;
    dockerCompat = true;
    dockerSocket.enable = true;
    defaultNetwork.settings.dns_enabled = true;
  };

  virtualisation.oci-containers.backend = "podman";
}

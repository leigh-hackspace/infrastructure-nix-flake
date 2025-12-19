{
  config,
  lib,
  ...
}:

let
  LIBRESPEED_UID = 8004;
in
{
  users.users.librespeed = {
    uid = LIBRESPEED_UID;
    group = "users";
    isNormalUser = true;
  };

  virtualisation.oci-containers.containers = {
    librespeed = {
      hostname = "librespeed";
      image = "ghcr.io/librespeed/speedtest:latest";
      autoStart = true;
      ports = [
        "${toString LIBRESPEED_UID}:8080"
      ];
      environment = {
        TZ = "Europe/London";
      };
    };
  };

  # journalctl -u anubis-default -f
  services.anubis = {
    defaultOptions.settings = {
      SLOG_LEVEL = "debug";
      OG_PASSTHROUGH = true;
      OG_EXPIRY_TIME = "1h";
    };

    instances.default = {
      settings = {
        TARGET = "http://127.0.0.1:${toString LIBRESPEED_UID}";
        BIND = "/run/anubis/anubis-default/anubis.sock";
        METRICS_BIND = "/run/anubis/anubis-default/anubis-metrics.sock";
      };
    };
  };

  # required due to unix socket permissions
  users.users.nginx.extraGroups = [ config.users.groups.anubis.name ];

  services.nginx.virtualHosts = {
    "speed.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://unix:${config.services.anubis.instances.default.settings.BIND}";
        recommendedProxySettings = true;
        proxyWebsockets = true;

        extraConfig = ''
          client_max_body_size 100M;
        '';
      };
    };
  };

  # services.nginx.virtualHosts = {
  #   "speed.leighhack.org" = {
  #     useACMEHost = "leighhack.org";
  #     forceSSL = true;

  #     locations."/" = {
  #       proxyPass = "http://127.0.0.1:${toString LIBRESPEED_UID}";
  #       recommendedProxySettings = true;
  #       proxyWebsockets = true;

  #       extraConfig = ''
  #         client_max_body_size 100M;
  #       '';
  #     };
  #   };
  # };
}

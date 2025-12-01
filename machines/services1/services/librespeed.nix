{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  constants = import ../config.nix;
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

  services.anubis.instances.default.settings.TARGET = "http://127.0.0.1:${toString LIBRESPEED_UID}";

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

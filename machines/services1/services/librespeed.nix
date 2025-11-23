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

  services.nginx.virtualHosts = {
    "speed.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:${toString LIBRESPEED_UID}";
        recommendedProxySettings = true;
        proxyWebsockets = true;

        extraConfig = ''
          client_max_body_size 100M;
        '';
      };
    };
  };
}

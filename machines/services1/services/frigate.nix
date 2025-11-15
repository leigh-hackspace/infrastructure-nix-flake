{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ../config.nix;
  mkSSOVirtualHost = import ../lib/nginx-sso-helper.nix;
in
{
  # sudo smem -r | sort -k 4 -nr | head
  # sudo smem -rs swap | head
  virtualisation.oci-containers.containers.frigate = {
    hostname = "frigate";
    image = "ghcr.io/blakeblackshear/frigate:stable";
    autoStart = true;
    volumes = [
      # "/srv/frigate/storage:/media/frigate"
      "/mnt/cameras/storage:/media/frigate"
      "/srv/frigate/config:/config"
      "/srv/frigate/certs:/etc/letsencrypt/live/frigate:ro"
    ];
    ports = [
      "5000:5000" # Insecure
      "8971:8971" # Secure
      "1984:1984" # go2rtc admin panel
      "8554:8554"
      "8555:8555/tcp"
      "8555:8555/udp"
    ];
    environment = {
      FRIGATE_RTSP_PASSWORD = "password";
    };
    extraOptions = [
      "--mount=type=tmpfs,destination=/tmp/cache,tmpfs-size=1000000000"
      "--device=/dev/dri/renderD128"
      "--device=/dev/apex_0:/dev/apex_0"
      "--shm-size=1024m"
      "--cap-add=CAP_PERFMON"
      "--privileged"
    ];
  };

  services.nginx.virtualHosts = {
    "frigate.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://10.3.1.20:5000";
    };

    "frigate.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.20:5000";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}

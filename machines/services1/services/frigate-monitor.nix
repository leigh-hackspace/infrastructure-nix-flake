{ config, ... }:

let
  CONFIG = import ../config.nix;

  AIBOX_IP = "10.3.1.32";
in
{
  # frigate-monitor on aibox (machines/aibox/frigate-monitor.nix):
  # scene-change monitor + web UI for the main_space camera. LAN-only;
  # the int record is synced by dns-sync.
  services.nginx.virtualHosts."frigate-monitor.int.leighhack.org" = {
    useACMEHost = "leighhack.org";
    forceSSL = true;

    locations."/" = {
      proxyPass = "http://${AIBOX_IP}:8090";
      recommendedProxySettings = true;
      extraConfig = CONFIG.LOCAL_NETWORK;
    };
  };
}

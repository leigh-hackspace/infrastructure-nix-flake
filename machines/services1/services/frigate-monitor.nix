{ config, lib, ... }:

let
  CONFIG = import ../config.nix;
  mkIntVhost = import ../lib/nginx-int-vhost-helper.nix { inherit lib; };

  AIBOX_IP = "10.3.1.32";
in
{
  # frigate-monitor on aibox (machines/aibox/frigate-monitor.nix):
  # scene-change monitor + web UI for the main_space camera. LAN-only;
  # the int record is synced by dns-sync.
  services.nginx.virtualHosts."frigate-monitor.int.leighhack.org" = mkIntVhost {
    proxyPass = "http://${AIBOX_IP}:8090";
  };
}

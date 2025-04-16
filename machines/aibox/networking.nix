{ config, pkgs, ... }:

{
  networking = {
    useDHCP = false;
    firewall.enable = false;
    networkmanager.enable = false;
  };

  systemd.network = {
    enable = true;
    wait-online.enable = false;

    links = {
      "10-lan" = {
        matchConfig = {
          MACAddress = "84:47:09:40:d2:80";
        };
        linkConfig = {
          Name = "lan";
        };
      };
    };

    netdevs = {
      "11-vlan225" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan225";
          MACAddress = "84:47:09:40:d2:25";
        };
        vlanConfig.Id = 225;
      };
    };

    networks = {
      "10-lan" = {
        matchConfig.Name = "lan";
        linkConfig.RequiredForOnline = "yes";
        networkConfig = {
          DHCP = false;
        };
        vlan = [
          "vlan225"
        ];
      };

      "11-vlan225" = {
        matchConfig.Name = "vlan225";
        networkConfig = {
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };
    };
  };
}

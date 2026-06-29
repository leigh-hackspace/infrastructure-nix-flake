{
  boot.kernel.sysctl = {
    # enable IPv4 and IPv6 forwarding on all interfaces
    "net.ipv4.conf.all.forwarding" = true;
    "net.ipv6.conf.all.forwarding" = true;

    "net.ipv4.conf.all.arp_filter" = 1;
    "net.ipv4.conf.default.arp_filter" = 1;
  };

  networking = {
    useDHCP = false;
    firewall.enable = false;
    networkmanager.enable = false;
    nftables.enable = true;
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

      "12-vlan227" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan227";
          MACAddress = "84:47:09:40:d2:27";
        };
        vlanConfig.Id = 227;
      };

      # Brigde needed for QEMU quests
      "13-br227" = {
        netdevConfig = {
          Kind = "bridge";
          Name = "br227";
          MACAddress = "84:47:09:40:d3:27";
        };
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
          "vlan227"
        ];
      };

      "11-vlan225" = {
        matchConfig.Name = "vlan225";
        networkConfig = {
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };

      "12-vlan227" = {
        matchConfig.Name = "vlan227";
        networkConfig = {
          DHCP = false; # Do not assign IP to vlan227 directly — use br227
        };
        bridge = [ "br227" ]; # Attach vlan227 to bridge
      };

      # Interface is disabled for now as server does not need an IP on VLAN 227. QEMU guests create their own interfaces i.e. "tap0"
      "13-br227" = {
        matchConfig.Name = "br227";
        networkConfig = {
          # DHCP = true;
          # IPv6AcceptRA = true;
          DHCP = false;
          IPv6AcceptRA = false;
        };
      };
    };
  };
}

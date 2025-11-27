{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ./config.nix;
in
{
  # # Enable networking
  # networking.networkmanager.enable = true;

  # # Or disable the firewall altogether.
  # networking.firewall.enable = false;

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
          MACAddress = "c8:d3:ff:a5:be:7c";
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
          MACAddress = "c8:d3:ff:a5:b2:25";
        };
        vlanConfig.Id = 225;
      };

      # "12-vlan227" = {
      #   netdevConfig = {
      #     Kind = "vlan";
      #     Name = "vlan227";
      #     MACAddress = "c8:d3:ff:a5:b2:27";
      #   };
      #   vlanConfig.Id = 227;
      # };
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
          # "vlan227"
        ];
      };

      "11-vlan225" = {
        matchConfig.Name = "vlan225";
        networkConfig = {
          DHCP = false;
          Address = [
            "10.3.1.20/24"
            "2001:8b0:1d14:225:d::1020/64"
          ];
          Gateway = ["10.3.1.1"];
          DNS = ["10.3.1.1"];
          IPv6AcceptRA = true;
        };
      };

      # "12-vlan227" = {
      #   matchConfig.Name = "vlan227";
      #   networkConfig = {
      #     DHCP = true;
      #     IPv6AcceptRA = true;
      #   };
      # };
    };
  };

  # This server handles HTTP traffic so we need to re-route to itself
  networking.extraHosts = ''
    10.3.1.20                 id.leighhack.org id.int.leighhack.org tailscale.leighhack.org
    2001:8b0:1d14:225:d::1020 id.leighhack.org id.int.leighhack.org tailscale.leighhack.org
  '';

  # Enable NAT
  networking.nat.enable = true;
  networking.nat.externalInterface = "vlan225";
  networking.nat.internalInterfaces = [ "wg0" ];

  networking.wireguard.interfaces = {
    # "wg0" is the network interface name. You can name the interface arbitrarily.
    wg0 = {
      # Determines the IP address and subnet of the client's end of the tunnel interface.
      ips = [ "10.47.3.20/16" ];
      listenPort = 51820; # to match firewall allowedUDPPorts (without this wg uses random port numbers)

      # This allows the wireguard server to route your traffic to the internet and hence be like a VPN
      # For this to work you have to set the dnsserver IP of your router (or dnsserver of choice) in your clients
      postSetup = ''
        ${pkgs.iptables}/bin/iptables -t nat -A POSTROUTING -s 10.47.0.0/16 -o vlan225 -j MASQUERADE
      '';

      # This undoes the above command
      postShutdown = ''
        ${pkgs.iptables}/bin/iptables -t nat -D POSTROUTING -s 10.47.0.0/16 -o vlan225 -j MASQUERADE
      '';

      # Path to the private key file.
      #
      # Note: The private key can also be included inline via the privateKey option,
      # but this makes the private key world-readable; thus, using privateKeyFile is
      # recommended.
      privateKeyFile = CONFIG.WIREGUARD_KEY_FILE;

      peers = [
        # For a client configuration, one peer entry for the server will suffice.

        {
          # Public key of the server (not a file path).
          publicKey = "gNxnNaKmHNOtA7n38aGdCl1KfD7X+c1CXZg/D89CYiY=";

          # Forward all the traffic via VPN.
          # allowedIPs = [ "0.0.0.0/0" ];
          # Or forward only particular subnets
          allowedIPs = [
            "10.47.0.0/16"
            "192.168.49.0/24"
          ];

          # Set this to the server IP and port.
          # endpoint = "ovh-nix.chrisdell.info:51820"; # ToDo: route to endpoint not automatically configured https://wiki.archlinux.org/index.php/WireGuard#Loop_routing https://discourse.nixos.org/t/solved-minimal-firewall-setup-for-wireguard-client/7577
          endpoint = "51.38.68.81:51820";

          # Send keepalives every 25 seconds. Important to keep NAT tables alive.
          persistentKeepalive = 25;
        }
      ];
    };
  };
}

{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: let
  CONFIG = import ../../common/config.nix;
in {
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
          # `10.3.1.36` is the IdP's address and it is load-bearing *as an address*.
          # The ProCurve switches have `radius-server host 10.3.1.36` in NVRAM, which
          # nobody rewrites (AGENTS.md rule 16), and `authentik.int.leighhack.org` — the
          # LDAPS endpoint aibox's sssd and the pi-room netboot image dial — is answered
          # from `common/dns-records.nix`'s `authentik` record, i.e. with this address.
          # When the authentik appliance was retired on 2026-10-28 the address therefore
          # moved here rather than the clients being reconfigured; see
          # docs/authentik-container-migration.md.
          #
          # Order matters: `.20` stays first so it stays the *primary* and everything
          # this box originates keeps sourcing from it.  That is also why the RADIUS and
          # LDAP outposts work at all from a second address — they are podman published
          # ports, and netavark's DNAT means conntrack sends the reply back out as the
          # address that was dialled (`machines/services1/services/authentik.nix` has the
          # note).  A RADIUS client's socket is connected: an Access-Challenge sourced
          # from `.20` is silently dropped, and the symptom is an auth timeout.
          #
          # The DHCP reservation for the old appliance's MAC is deliberately still in
          # `common/dns-records.nix`, which is why the VM must stay *stopped* in
          # Proxmox's HA (`ha-manager set vm:108 --state stopped`): a booted VM takes
          # this address back and the resulting conflict is silent for whoever logs in.
          Address = [
            "10.3.1.20/24"
            "10.3.1.36/24" # the IdP: RADIUS 1812/udp, LDAP 389/636, see above
            "2001:8b0:1d14:225::20/64" # Public
            "2001:8b0:1d14:225:d::1020/64" # LEGACY TO BE DELETED
            "fd99:dead:beef:225d::20/64" # NATv6
          ];
          Gateway = ["10.3.1.1"];
          DNS = ["10.3.1.1"];
          IPv6AcceptRA = true;
        };
        # Accept the RA for routing/gateway but don't autoconfigure addresses
        ipv6AcceptRAConfig = {
          UseAutonomousPrefix = false; # suppress SLAAC — no new addresses from RA prefixes
          UseOnLinkPrefix = true; # still treat the prefix as on-link (routing works)
          UseGateway = true; # install the RA default route (connectivity)
          UseDNS = false; # optional: ignore RA-advertised DNS if you prefer yours
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

  # Names that used to be re-routed to this box because it was the TLS terminator.
  # That is the front door's job now (`machines/gw/http.nix`), so `id.leighhack.org`
  # and `tailscale.leighhack.org` must resolve through the network's DNS instead of
  # being pinned here: a client on this box dialling `id.leighhack.org` now reaches the
  # door, which proxies to the appliance.  The two `unifi.int` entries stay — that is a
  # different controller on 10.3.1.21, not a name this box used to terminate.
  networking.extraHosts = ''
    10.3.1.21                 unifi.int.leighhack.org
    2001:8b0:1d14:225:d::1021 unifi.int.leighhack.org
  '';

  # Enable NAT
  networking.nat.enable = true;
  networking.nat.externalInterface = "vlan225";
  networking.nat.internalInterfaces = ["wg0"];

  networking.wireguard.interfaces = {
    # "wg0" is the network interface name. You can name the interface arbitrarily.
    wg0 = {
      # Determines the IP address and subnet of the client's end of the tunnel interface.
      ips = ["10.47.3.20/16"];
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
          # Or forward only particular subnets.
          #
          # `192.168.49.0/24` used to be here as well, and that was wrong once the
          # Grafton site-to-site moved to the tailnet: the peer has never
          # handshook (`wg show` reports `0 B received`), so claiming the subnet
          # gave this box a route into a dead tunnel and made it the one host on
          # the LAN that could not reach Grafton.  The route lives on gw now
          # (`machines/gw/uplink.nix` accepts the tailnet route advertised by
          # `grafton-hackspace-client`), and every LAN client — this one included —
          # reaches it through the default gateway.
          allowedIPs = [
            "10.47.0.0/16"
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

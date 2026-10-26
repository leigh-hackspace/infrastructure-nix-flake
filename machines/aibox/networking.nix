let
  # The device table the router renders its DNS from.  Read, not copied: the
  # addresses below are the ones `aibox.int.leighhack.org` answers with, and a
  # second spelling of them here is how the two drift apart.
  RECORDS = import ../../common/dns-records.nix;
in {
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

          # The IPv6 addresses this box is *named* by, configured here because
          # nothing else on the network hands them out.
          #
          # `common/dns-records.nix` is what `machines/gw/dns.nix` renders into
          # dnsmasq's hosts file, so `aibox.int.leighhack.org` answers with these
          # two addresses whether or not any interface holds them — and since the
          # 2026-10-21 cutover nothing assigns them: the retired appliance honoured
          # them as one of fourteen per-host static DHCPv6 bindings (no `range6`
          # anywhere, so never a pool) and `machines/gw/dns.nix` does neither that
          # nor the NPT66 the `*d::/64` addresses leaned on
          # (`option6:23,[::]`, `ra-only` ranges), so the network advertises IPv6
          # and leases nothing.  What SLAAC would give this box instead is its
          # EUI-64 address
          # (`2001:8b0:1d14:225:8647:9ff:fe40:d225`), which is in no table and
          # answers no name.
          #
          # For most clients a dead AAAA is a shrug — a Happy-Eyeballs browser
          # falls back to the A record after one failed connect.  For a PXE client
          # it is a hang: iPXE on the Pi Room boards SLAACs, prefers the AAAA, and
          # sits retransmitting a SYN to an address nobody owns until it times the
          # transfer out (`Connection timed out`, https://ipxe.org/4c0a6092) and the
          # boot dies before it has shown a menu.  That was the whole of the
          # "netboot stopped working when the router moved" report of 2026-10-26:
          # nginx on this box was serving every byte, and the last successful boot
          # in its access log is 2026-10-21 18:45 — the evening the appliance was
          # retired, with this box still holding the address the old DHCPv6 server
          # had given it.  Nothing has handed it out since (this box next rebooted
          # on 2026-10-23), and nothing reclaims it either: the address simply
          # stopped existing.  `machines/pi-room-sys/README.md` has the
          # reproduction.
          #
          # Both families are kept: the SLAAC address stays (it is what the box has
          # been reachable at since the cutover) and these become the named ones.
          # The /64 is the VLAN's own prefix length, the same one
          # `machines/gw/networking.nix` puts on the router address.
          Address = map (a: "${a}/64") RECORDS.aibox.ipv6;

          IPv6AcceptRA = true;
        };
      };

      "12-vlan227" = {
        matchConfig.Name = "vlan227";
        networkConfig = {
          DHCP = false; # Do not assign IP to vlan227 directly — use br227
        };
        bridge = ["br227"]; # Attach vlan227 to bridge
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

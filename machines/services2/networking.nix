# services2 LAN wiring.
#
# This box is headless and lives on VLAN 225 with the rest of the services
# boxes: the physical Intel I211 NIC (`enp8s0`, MAC 70:85:c2:53:f1:d4) is the
# untagged trunk uplink and carries no address of its own; all addressing is on
# the tagged `vlan225` child.
#
# Unlike services1, the IPv4 address is **not** configured here: it comes from
# the router's DHCP reservation for the NIC MAC (10.3.1.21).  The matching
# OPNsense host override is `services2` in /conf/config.xml (dnsmasq), which
# also publishes services2 / services2.int.leighhack.org.  IPv6 uses RA/SLAAC
# from the router (same as aibox's VLAN 225 interface).
#
# NOTE: the switch port this NIC is plugged into must carry VLAN 225 tagged
# (a trunk), exactly like services1's port — an access port would leave vlan225
# with no address and the box unreachable.
{
  config,
  lib,
  pkgs,
  ...
}: {
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
          MACAddress = "70:85:c2:53:f1:d4";
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
        vlan = ["vlan225"];
      };

      "11-vlan225" = {
        matchConfig.Name = "vlan225";
        networkConfig = {
          # Static 10.3.1.21 on the router, keyed on the NIC MAC above.
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };
    };
  };
}

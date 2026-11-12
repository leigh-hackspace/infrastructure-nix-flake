# PARKED — 2026-11-11 (5787f23) — machines/services3/README.md ("The move to VLAN 225")
# — re-verify with: on the racked box, import this file and drop ./networking.nix from
# machines/services3/default.nix, then `sudo nixos-rebuild dry-activate --flake
# ".#services3"` and, after the switch, `nmcli device status` (no `wlp*` carrier),
# `ip -br addr show vlan225` answering the wired MAC's reservation, and
# `getent hosts services3.int.leighhack.org` answering that same new address.  Do not
# enable it while the box is on the bench: it removes the only uplink the box has.
#
# The wired VLAN 225 wiring services3 wants when it is racked, written now because the
# hardware decision was made before the box could be cabled, and because the two
# mistakes available here are both outages: leaving NetworkManager on while networkd
# configures the same NIC (two DHCP clients, one lease, whichever answers last), and
# adding the row to `common/hosts.nix` without the reservation in
# `common/dns-records.nix` (which `machines/gw/lib/check-network-tables.nix` refuses,
# loudly, and correctly).
#
# This is `machines/services2/networking.nix`'s shape on purpose — rename the NIC that
# carries the network to `lan`, hang a tagged `vlan225` child off it, and take the
# address from the router's reservation — with two differences that are about this box:
# the match is on the onboard I219-V's MAC, and NetworkManager is turned off rather
# than left configured for a dongle that is going back in a drawer.
{lib, ...}: {
  networking = {
    useDHCP = false;
    firewall.enable = false;

    # The dongle is the reason ./networking.nix exists at all.  On the rack it is not
    # the uplink, and NM and networkd must not both own a NIC.
    networkmanager.enable = lib.mkForce false;
  };

  systemd.network = {
    enable = true;
    wait-online.enable = false;

    links = {
      "10-lan" = {
        # The onboard Intel I219-V, measured 2026-11-11 with `ip -br link` while it had
        # no cable in it.  Re-measure on the rack: if the board is ever swapped, or the
        # BIOS renames it, this match silently produces no `lan` device and the box is
        # unreachable by its own address.
        matchConfig = {
          MACAddress = "70:8b:cd:a8:50:bc";
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
          # The address comes from the router, keyed on the MAC above — so the
          # reservation in common/dns-records.nix is *this* box's second half, and the
          # switch port has to carry VLAN 225 tagged (a trunk), exactly like services1's
          # and services2's ports.  An access port here leaves vlan225 with no address.
          DHCP = true;
          IPv6AcceptRA = true;
        };
      };
    };
  };

  # And then the two tables move together, in the same change, on two different
  # machines: `common/dns-records.nix` grows the wired MAC and drops the dongle's,
  # `common/hosts.nix` changes the address, and `gw` has to be deployed for the
  # reservation to be the router's rather than this flake's opinion.  ./README.md has
  # the order, because the order is the part that can strand the box.
}

# services3's wired VLAN 225 wiring — **deployed 2026-11-25** (`git log -1 --
# machines/services3/vlan225.nix` is the commit), when the box came off the bench's
# Wi-Fi and took a cable into the wired network.  It replaced
# `./networking.nix` (NetworkManager and a USB dongle) in the same change that moved
# the row in `common/dns-records.nix` and the address in `common/hosts.nix`; that file
# is **deleted**, not parked — the box has one uplink now, and a parked Wi-Fi module
# beside a wired box is the half-move AGENTS.md rule 23 exists to prevent (`git
# log --diff-filter=D -- machines/services3/networking.nix` recovers it, and
# `machines/services3/README.md` owns the story).
#
# What was measured on the box before this file went in, because every failure mode
# here is "unreachable":
#
#   * `ip -br link enp0s31f6` — `UP ... <BROADCAST,MULTICAST,UP,LOWER_UP>` and MAC
#     `70:8b:cd:a8:50:bc`, i.e. **a carrier**, which is what changed since the 2026-11-11
#     read (`DOWN ... <NO-CARRIER>`) and what says the cable is in.
#   * `ip -s link` — RX 1045 packets / TX 120, so the far end of that cable is a network
#     that talks, not a dead conduit.
#   * the tag chain, proven before any reservation existed: with a VLAN-225 child
#     configured against the **real** NIC name, the router answered the NIC with
#     `DHCPv4 address 10.3.1.248/24, gateway 10.3.1.1 acquired from 10.3.1.1`.  A lease
#     out of VLAN 225's own dynamic range whose gateway is the router is only possible
#     if VLAN 225's tag arrives down that cable, so the port is a trunk and not an
#     access port — which is exactly the one physical fact this file cannot state.
#
# The re-verify the parked notice asked for, kept: it named `getent hosts
# services3.int.leighhack.org`, and that name **does** answer — measured on gw
# 2026-11-25 it prints `10.3.1.22  services3.int.leighhack.org`.  `common/dns-records.nix`
# is one dnsmasq record (`dhcp-host=… ,services3`) inside the zone `int.leighhack.org`,
# and `dhcp-fqdn` hands the lease out again under `<name>.<zone>`, so the record's name
# exists both bare and qualified, and neither form is a row in `common/int-dns.nix` or
# `common/front-door.nix` (the engine's own name, `strata3.int`, is that row — rule 24).
# What **is** true, and is the trap instead: `getent hosts services3` run **on the box
# that owns the name** answers `127.0.0.2` — NixOS writes `127.0.0.2  <machine-name>` into
# every flake box's own `/etc/hosts` (measured on services1 and on services3 2026-11-25;
# the SSH section of AGENTS.md has carried this as an aibox-only "gotcha" for a year) — so
# a box can never ask itself what its name answers.  Re-verify from **gw** instead: `getent
# hosts services3` and `getent hosts services3.int.leighhack.org` both answering `10.3.1.22`,
# `grep services3 /etc/dnsmasq.d/network.conf` (the reservation line), and on the box
# `ip -br link` (`lan` plus `vlan225@lan`, both `UP`, carrying `10.3.1.22/24`).
#
# **A switch is not enough on this box: it wants a boot.** `systemd.network.links`
# renames the NIC to `lan`, but udev applies a `.link` only when the device is
# (re)created, so a switch alone leaves the device called `enp0s31f6` and every file
# below that matches `lan` matches nothing — measured on 2026-11-25, where the switch
# wrote `/etc/systemd/network/10-lan.{link,network}` and `networkctl` still printed
# `enp0s31f6 ... unmanaged`, no `vlan225`.  The hand-written test that matched the real
# name is what proved the VLAN itself was fine, and the reboot is what makes the
# flake's own files apply.  One piece of boot-time noise is expected, not a fault: the
# `.link` rule matches by **MAC**, and a VLAN child carries its parent's MAC, so udev also
# tries to rename the child — `vlan225: Failed to rename network interface 3 from 'vlan225'
# to 'lan': File exists`, followed by `ignoring`.  The rename is refused, the child keeps its
# kernel name and the `vlan225@lan` alias, and `networkctl` then reads `vlan225 … routable
# (configured)`.  Do not "fix" that by matching the link rule on a name instead of the MAC:
# the MAC is what makes the rule survive a kernel rename, and the refusal is what keeps the
# child addressable at all (`10.3.1.22/24 … dynamic vlan225`).  Do not "fix" this by matching
# `enp0s31f6` here and dropping the rename: services1 and services2 both use the renamed-parent shape, and a board
# swap or a BIOS rename changes the device name and not the MAC, so matching the MAC is
# the only stable handle.
{lib, ...}: {
  networking = {
    useDHCP = false;
    firewall.enable = false;

    # The dongle is gone, so nothing here needs NetworkManager — and this stays `mkForce`,
    # not a plain `false`, because the thing it refuses is a *future* re-add: NixOS files
    # in `/etc/NetworkManager/system-connections` are not managed by this flake, so a
    # dongle put back at a bench would be an unmanaged credential file whose daemon would
    # race the networkd config below for one NIC (two DHCP clients, one lease).  Refusing
    # that co-existence is this module's job, not a comment's.
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

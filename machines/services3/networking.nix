# services3's wiring, as measured on the bench on 2026-11-11.
#
# THIS IS THE ONE SERVICES BOX THAT IS NOT ON THE WIRED LAN, and the file says so in
# its first line so that nobody reads `common/hosts.nix`'s `services3` as a 10.3.1.x
# address.  The box is on the bench, its wired I219-V (`enp0s31f6`, MAC
# 70:8b:cd:a8:50:bc) has no cable — `ip -br link` reports `DOWN ...
# <NO-CARRIER,...>` — and everything it does goes through a USB Wi-Fi dongle:
#
#   wlp0s20f0u1  90:de:80:e1:4e:42  Realtek RTL8821CU (0bda:c811), driver rtw88_8821cu
#                10.3.2.213/24  VLAN 226  gateway 10.3.2.1  DNS 10.3.2.1 (+2001:8b0:1d14:226::1)
#                search domain `int.leighhack.org`, from the router's DHCP
#
# ...so the address is the **dongle's**, and `common/dns-records.nix` pins the
# **dongle's MAC**.  Nothing here configures an address: the router's reservation for
# that MAC is what makes 10.3.2.213 stable, exactly as machines/services2/networking.nix
# relies on its own reservation for 10.3.1.21.  IPv6 is SLAAC out of VLAN 226's
# advertised /64 (common/vlans.nix), which is why `common/dns-records.nix`'s `ipv6`
# for this box is `[]`: the two addresses it holds are temporary and owned by no name.
#
# WHY NetworkManager and not systemd-networkd like the other services boxes: not
# preference, but the shape of the uplink.  The Wi-Fi connection is one file,
# `/etc/NetworkManager/system-connections/WiFi Go Brrr.nmconnection` (root, 0600),
# created by hand at the bench when the box arrived — the PSK is in it, and it is the
# only copy of the credential that reaches the network this box is currently on.
# NixOS does not manage files in that directory, so a switch leaves it alone, which is
# what makes this box safe to switch at all from the far end of that same connection.
# Enabling `systemd.network` here instead would have left the box with no uplink and
# no console, i.e. a bench visit, for no gain: on the move to VLAN 225 this whole file
# comes out and ./vlan225.nix goes in (./README.md is the procedure).
#
# The PSK is deliberately NOT brought into secrets/secrets.yaml: it is a bench
# convenience for a box that is leaving this VLAN on purpose, and every name added to
# that file is a credential that gets decrypted on every box that builds.  If this box
# is still on Wi-Fi when someone next needs it, the fix is a `sops.secrets` entry plus
# `networking.networkmanager.connections` (which renders to
# /run/NetworkManager/system-connections and so does not fight the /etc file), not a
# pasted PSK.
{...}: {
  networking = {
    # The firewall stays off, as on every other box here: the network's filtering is
    # the router's nftables, rendered from common/config.nix, and a second stateful
    # firewall on a service box has only ever been the thing that made a service
    # unreachable in a way its own config did not explain.
    firewall.enable = false;

    # `networking.useDHCP` is left alone: nixpkgs' NetworkManager module sets it to
    # false itself, and spelling it out here would be a second answer to a question
    # the module has already answered.
    networkmanager.enable = true;
  };
}

# DATA — plain attrset read by `import` from anywhere in the tree; it is not a module and defines no options.
# The machines in this network and the address each one answers on.
#
# One table so the nginx vhost proxies, the Prometheus scrape targets and the
# gatus checks cannot disagree about where a box lives.  Every entry is a fixed
# address: the DHCP reservation on the router (see docs/router-and-dns.md), not a DNS name,
# so nothing here depends on the resolver it is meant to be independent of.
{
  services1 = "10.3.1.20";
  services2 = "10.3.1.21";
  # Wired, with the others, as of 2026-11-25: the bench's cable went in and the box took
  # VLAN 225, so it lives in the services block like services1 and services2.  The
  # address is reserved in ./dns-records.nix on the board's own I219-V MAC
  # (`70:8b:cd:a8:50:bc`), and 10.3.1.22 is outside VLAN 225's dynamic range
  # (10.3.1.150-.253), so unlike the old row this reservation does not cost the pool one
  # of its own addresses.  That the port is a *trunk* rather than an access port is
  # measured, not assumed: with VLAN 225 switched on and before any reservation existed,
  # the NIC took a lease in that dynamic range whose gateway the router named as
  # `10.3.1.1` (`DHCPv4 address 10.3.1.248/24, gateway 10.3.1.1 acquired from 10.3.1.1`),
  # which is the only way to learn it — see `machines/services3/README.md`.
  #
  # What went with the move is the Wi-Fi dongle: `machines/services3/networking.nix`
  # (NetworkManager and the dongle's PSK) is deleted, `./vlan225.nix` is the wiring, and
  # the dongle's MAC is gone from ./dns-records.nix so the table says one thing about a
  # machine that has one address.  Nothing in this flake dials 10.3.2.213 any more;
  # the receipts for the old arrangement are history (`git log -- common/hosts.nix`).
  services3 = "10.3.1.22";
  aibox = "10.3.1.32";
  router = "10.3.1.1";
  nas = "10.3.1.6";
  # The three ProCurve switches. Not flake machines and not host entries either:
  # they are managed over Telnet, which is why
  # machines/services1/services/switch-gui.nix proxies them over HTTPS.
  # Fixed addresses like everything else here — switch-gui must not depend on the
  # DNS it is also used to debug.
  #
  # The names are the switches' own `System Name` (`show system`), which is also
  # what switch-gui's URLs use (`/switch/switch2`), so this table, the GUI and
  # the gatus checks cannot disagree about which switch is which.
  switch1 = "10.3.1.2"; # J9280A 2510G-48, Y.11.44 — the core switch
  switch2 = "10.3.1.3"; # J9280A 2510G-48, Y.11.44
  switch3 = "10.3.1.4"; # J4903A 2824, I.10.107 — Workshop
}

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
  # Not with the others, and read that before trusting the name: services3 is on the
  # bench, and its only uplink is a USB Wi-Fi dongle, so this is the dongle's lease on
  # VLAN 226 (the wireless block) rather than a wired address in 10.3.1.x.  It is the
  # same fact as every other row here — a fixed address, reserved by MAC in
  # ./dns-records.nix — reached by a different physical path, which is why the deploy
  # and the scrape targets work unchanged and why `machines/services3/networking.nix`
  # keeps NetworkManager.  Racking it moves this line (procedure:
  # `machines/services3/README.md`, "The move to VLAN 225").
  services3 = "10.3.2.213";
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

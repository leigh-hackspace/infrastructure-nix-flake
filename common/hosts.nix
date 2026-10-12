# The machines in this network and the address each one answers on.
#
# One table so the nginx vhost proxies, the Prometheus scrape targets and the
# gatus checks cannot disagree about where a box lives.  Every entry is a fixed
# address: the DHCP reservation on the router (see AGENTS.md), not a DNS name,
# so nothing here depends on the resolver it is meant to be independent of.
{
  services1 = "10.3.1.20";
  services2 = "10.3.1.21";
  aibox = "10.3.1.32";
  router = "10.3.1.1";
  nas = "10.3.1.6";
  # The ProCurve 2510G-48 core switch (J9280A, Y.11.44).  Not a flake machine
  # and not a host entry either: it is managed over Telnet, which is why
  # machines/services1/services/switch-gui.nix proxies it over HTTPS instead.
  # Fixed address like everything else here — switch-gui must not depend on the
  # DNS it is also used to debug.
  switch = "10.3.1.2";
}

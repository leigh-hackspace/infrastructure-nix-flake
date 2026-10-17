# The interface names this box's config keys on, in one place because three files
# read them: ./hardware-configuration.nix creates the two NIC names with `links`
# rules, ./networking.nix hangs the VLAN devices off `LAN_NIC` and puts an address
# on `UPLINK_NIC`, and ./firewall.nix masquerades out of `UPLINK_TUNNEL`.  A name
# written twice is two facts that can be moved apart, and the failure they move
# apart into is a router that answers nothing — or, for `UPLINK_TUNNEL`, a router
# that routes everything with its real source address and gets no replies.
#
# The two NIC names are *names* rather than MACs because a VLAN device has no MAC
# of its own to match on, so everything on the LAN side is keyed by name and the
# one place a MAC appears is the `links` rule that creates the name — those rules
# are in ./hardware-configuration.nix, pinned to the two e1000e MACs measured on
# the box.  `UPLINK_TUNNEL` is the one name here the flake cannot create: pppd
# makes it at runtime, and ./README.md says to read it back off a box that has
# dialled.
{
  LAN_NIC = "router-lan"; # the NIC the VLAN devices hang off

  # The NIC the uplink arrives on — the AAISP hub's handoff, which the old box
  # called `enc0`.  It is a *physical* NIC and it takes its address from the hub
  # (./networking.nix), which is the fact that decides whether the tunnel below
  # can dial at all.
  UPLINK_NIC = "router-uplink";

  # The interface the tunnel itself creates, which is not the NIC above: pppd
  # makes this one at runtime and the flake cannot name it.  `ppp0` is the name
  # the kernel gives the first ppp unit, so it is the honest guess, and it is the
  # name ./firewall.nix's masquerade rule matches — read it off the real box with
  # `ip link` before trusting that rule (./README.md, "read the rendered file").
  UPLINK_TUNNEL = "ppp0";
}

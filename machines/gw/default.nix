flakeInputs: {
  # The name the network has always called this box: `common/hosts.nix` calls
  # its address `router`, and the old OPNsense box's hostname was `gw`.  Keep
  # it — the ssh known-hosts file in /etc/dns-sync/router-known-hosts and the
  # nginx vhost that used to reach the old box's UI are keyed on it.
  networking.hostName = "gw";

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix

    ./dns.nix
    ./firewall.nix
    ./networking.nix
    ./network-status.nix
    ./openvpn.nix
    ./sops.nix
    ./uplink.nix

    # The assertions over common/vlans.nix and common/dns-records.nix.  This
    # module is the only thing in this machine's list that cannot be "fixed by
    # deploying": a reservation that names a VLAN the router does not create,
    # or two devices claiming one address, is invisible until a client gets the
    # wrong address — and deploying would not show it, because the rendered file
    # is generated from the same tables and so says the same wrong thing.  The
    # check is over the tables, not over the rendering.  `nix flake check` runs
    # it, and `just check` runs the standalone version
    # (./lib/check-network-tables.nix) so the same assertions are readable
    # without a deploy.
    ./lib/assert-network-tables.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}

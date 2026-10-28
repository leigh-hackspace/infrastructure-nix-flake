{
  # `flakeInputs` arrives through specialArgs — flake.nix's `extraArgs` — for the one
  # line that needs it (`flakeInputs.sops-nix.nixosModules.sops` near the bottom: a
  # dashed attribute name cannot be a function-argument pattern, which is why this file
  # used to be `flakeInputs: {…}` applied at flake.nix's call site).
  flakeInputs,
  ...
}: {
  # The name the network has always called this box: `common/hosts.nix` calls its
  # address `router`, and the old OPNsense box's hostname was `gw` — so every
  # client's known_hosts entry, `gw.int.leighhack.org` (this box's own
  # `networking.extraHosts` line) and the headscale node name are keyed on it.
  # Changing it is a network-wide act, not a rename.
  networking.hostName = "gw";

  # Build on aibox, like services1 does.  This box is the network's router: a
  # compile here is four `rustc` processes on the machine that answers DNS and DHCP
  # for every device in the building, which is what happened on 2026-10-21 when a
  # `just check` (`fmt-check` → `nix develop`) put a wasm-bindgen-cli build on the
  # gateway.  `common/build-farm.nix` turns this option into `nix.distributedBuilds`
  # plus `/etc/nix/machines`, and nix 2.34 *prefers* the external builder for every
  # derivation — including the sops-secret derivations built during evaluation, whose
  # delegated build is the reason that module also appends `leigh-admin` to
  # `trusted-users` (already set by hand in ./configuration.nix, where the push path's
  # need for it is explained; the two lists simply merge).
  #
  # **`nix.settings.max-jobs = 0` is the part that makes the rule mechanical.**  Remote building
  # prefers the builder and *falls back to local in silence* when no builder can take
  # the job — so a builder alone still leaves the failure mode, just rarer (aibox
  # down, aibox out of disk, one unrepresentable derivation).  With the local daemon
  # holding zero jobs there is nothing to fall back to: the build fails, loudly, and
  # the router keeps answering DNS. `common/build-farm.nix`'s note on `--max-jobs 0`
  # says as much ("it removes the local fallback"; with `distributedBuilds` off the
  # same setting fails with "local builds are disabled").  Substitution is unaffected
  # — copying a path in is not building one — so a switch on this box (its own, or
  # `just deploy gw` from elsewhere) and every rollback continue to work with aibox
  # unreachable.
  #
  # Garbage collection is the other half of the module and it stays off here: a router
  # that walks and rewrites its own store on a timer is an unasked-for second opinion
  # about the rollback profile (`system-good` is its own GC root, so collection is
  # *probably* safe, which is not the same as a reason to have it on the one box that
  # must not be busy at 04:00).  The build-user invariant still applies, from
  # common/base.nix's own import of common/nixbld.nix.
  #
  # None of this makes building here *correct* — `just switch` here (or `just deploy gw`
  # from another box) is the way, and `_build-host-guard` still refuses the build-shaped
  # recipes on this host, because evaluation is still paid on this box and a guard is
  # cheaper than an explanation afterwards.  What it buys is that the accident — a stray
  # `nix build`, an agent reaching for `nix develop` — is now aibox's problem and not
  # the network's.
  infra.buildFarm.builderHost = (import ../../common/hosts.nix).aibox;
  infra.buildFarm.gc = false;
  # The renamed form of the option: `nix.maxJobs` still works but is a deprecated
  # alias, and this line is load-bearing — it is what removes the silent local
  # fallback — so it must not turn into a hard error the day nixpkgs deletes the
  # alias.  `nix config show` prints the setting as `max-jobs` either way.
  nix.settings.max-jobs = 0;

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix

    ./dns.nix
    ./firewall.nix
    # headscale + headplane: the network's tailnet control server and admin UI, moved
    # here from services1 with the front door. The daemons bind 8085/8086 on loopback;
    # `tailscale.leighhack.org` and `headplane.leighhack.org` are rows in
    # common/front-door.nix.
    ./headscale.nix
    # The router as a TLS endpoint: nginx, the `int.leighhack.org` certificate and
    # the first two `*.int` vhosts.  Phase 1 of docs/router-front-door-migration.md,
    # and the file's header is the one that says what it deliberately does not do.
    ./http.nix
    ./networking.nix
    ./network-status.nix
    # The two things its `/api/connections` table needs from the machine itself:
    # the conntrack accounting sysctl, and group read access to
    # /proc/net/nf_conntrack for a DynamicUser.  Deployed with it, not inside it,
    # because both have to hold before the service's first probe.
    ./conntrack-reader.nix
    # What its `/api/leases` table needs: dnsmasq's lease file is `0640 root:root`,
    # and this is the module that decides who may read it (and keeps it readable
    # when dnsmasq rewrites it).  It reuses the account and group that
    # ./conntrack-reader.nix defines, so it is imported after it for the reader's
    # sake rather than for the module system's.
    ./lease-reader.nix
    ./openvpn.nix
    ./sops.nix
    ./uplink.nix

    # The guard over common/int-dns.nix read from *this box's nginx*: the same table
    # ./dns.nix answers with, compared against the vhosts ./http.nix declares.  It is
    # a module rather than a `just check` evaluator because it needs a config, and it
    # is the check that cannot be fixed by deploying — same argument as the file
    # below, in the one place on this machine where the two sides of a comparison
    # live on two boxes.  AGENTS.md rule 24: a vhost move is a guard move.
    ./int-dns.nix

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

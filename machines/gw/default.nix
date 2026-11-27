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

  # **This box builds for itself, and is bounded while doing it.** Until 2026-11-23 it
  # delegated every build to aibox and held `nix.settings.max-jobs = 0`, so that nothing
  # could ever compile here; that arrangement is withdrawn (common/nix-gc.nix's header is
  # the why), which leaves the original problem — this is the network's router, and a
  # compile here is `rustc` on the machine that answers DNS and DHCP for every device in
  # the building, which is what happened on 2026-10-21 when a `just check`
  # (`fmt-check` → `nix develop`) put a wasm-bindgen-cli build on the gateway — to be
  # solved by a **bound** instead of by a builder.
  #
  # Two bounds, and they do different jobs:
  #
  # * `nix.settings.max-jobs = 2` on a 4-thread box: a switch here compiles its own
  #   `network-status` crate and its embedded wasm bundle (common/crane-args.nix), so it
  #   costs minutes rather than nothing, and it cannot take the last core from dnsmasq.
  # * `nix-daemon.service` capped at 3 GiB (`MemoryHigh`, throttled) / 4.5 GiB
  #   (`MemoryMax`, refusing) out of this box's 7 GiB: builds run as children of the
  #   daemon, so that cgroup is where a runaway `rustc` lands. Without it the OOM killer's
  #   biggest target on this box is not the build, it is nginx/headscale/dnsmasq under
  #   load — and aibox's own strata failures (machines/aibox/STRATA.md, 2026-11-23) are
  #   the measured version of what an uncapped build does to the thing sharing its RAM.
  #
  # What is *not* bounded: substitution. Copying a path in is not building one, so a
  # switch here, a `just deploy gw` from elsewhere, and every rollback keep working with
  # no builder anywhere — which is the property the old `max-jobs = 0` was protecting, and
  # it is still true.
  #
  # Garbage collection stays off here: a router that walks and rewrites its own store on a
  # timer is an unasked-for second opinion about the rollback profile (`system-good` is its
  # own GC root, so collection is *probably* safe, which is not the same as a reason to
  # have it on the one box that must not be busy at 04:00). The build-user invariant still
  # applies, from common/base.nix's own import of common/nixbld.nix.
  #
  # Self-building does not make building here *correct*. `just check` still skips its
  # flake step on gw out loud, and `_build-host-guard` still refuses the build-shaped
  # recipes on this host: the router's gate is `just check-eval`, and a five-toplevel
  # `just check-build` or a crate test suite on 7 GiB is not a thing to discover
  # afterwards. What the two bounds buy is that the accident — a stray `nix build`, an
  # agent reaching for `nix develop` — is survivable rather than network-wide.
  infra.nixGc.enable = false;
  # The renamed form of the option: `nix.maxJobs` still works but is a deprecated
  # alias, so it is spelled the current way; `nix config show` prints it as `max-jobs`
  # either way. Load-bearing in the opposite direction from before 2026-11-23: it is now
  # the ceiling on what a build here costs, not the thing that forbids one.
  nix.settings.max-jobs = 2;
  systemd.services.nix-daemon.serviceConfig = {
    MemoryHigh = "3G";
    MemoryMax = "4500M";
  };

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix

    ./dns.nix
    # The replacement for ./dns.nix, rendered from the same tables and **disabled**:
    # this file is the cutover switch and the eval-time check that the render is
    # complete (its assertion forces the whole document).  `docs/netd-2026-11-26.md`
    # is the plan and the runbook; `just netd-render` prints what it would hand the
    # daemon, without a build and without touching a socket.
    ./netd.nix
    # The IPv6 router advertisements dnsmasq sends, and what happens when its timer
    # dies with a `router-lan` link flap: a check that counts the advertisements on
    # the wire, restarts dnsmasq when they stop, and says so on Slack as well as in
    # this box's unit state.  Three silent outages (2026-10-21, 2026-11-19,
    # 2026-11-26) are why it exists; the header is the measurement that rules out the
    # counter `docs/follow-ups.md` suggested.
    ./ra-watch.nix
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
    # The WAN supervisor: `crates/wan-manager` discovers every possible uplink
    # (non-LAN DHCP NICs, USB-tethered phones, any number of wireless adapters),
    # measures and ranks them, and repairs the routes and the firewall's
    # `wan_ifaces` NAT set every 5 s. The 2026-11-07 AAISP outage is why the
    # backup exists; `docs/wan-manager-2026-11-09.md` owns the current policy,
    # the Android USB diagnosis and the limits.
    ./wan-backup.nix
    ./wan-relay.nix
    # The UPS on this box's USB port, and the ordered power-off it drives: NUT for
    # the protocol, `upssched` for the outage countdown, and `crates/ups-watchdog`
    # for the two things neither knows how to do — broadcast a notice to every
    # server, and power the fleet off before itself.  The grace period, the notice
    # texts and the target list all live there; `docs/ups-and-fleet-shutdown.md`
    # has the battery maths and what is measured rather than assumed.
    ./ups.nix

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

    # pi's endpoint list (docs/pi-agent-models.md). Yes, on the router: the front door
    # is what injects the Strata key, so the box that terminates every inference
    # session is also a place a person may want to run pi, and rule 22 is respected —
    # the crate is built on aibox and substituted here, exactly like
    # network-status and wan-manager.
    ../../common/pi-agent.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];

  programs.pi-agent.enable = true;
}

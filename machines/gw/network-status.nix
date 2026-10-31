# network-status: the router's own network dashboard, hosted on gw.
#
# This is gw's /proc and /sys rendered as charts: per-interface bandwidth, the
# nftables conntrack table, CPU/memory/load and "potential issues", plus the
# same data as Prometheus metrics.  The crate is `crates/network-status` (zero
# external crates); its Dioxus SPA in `frontend/` is built by the flake and
# **embedded in the binary** (`build.rs`, `common-rs/build-spa`) rather than served
# out of a directory, so there is no `dist/` to commit — see
# `crates/network-status/frontend/README.md`.
#
# It used to run on services1 and ssh to gw as `root`, first talking to the
# OPNsense box and then briefly to gw.  Hosting it here is the last step of that
# port: the probe is `--local`, so this unit is a DynamicUser reading kernel
# files — no login, no ssh key, no host-key pin.  Two of the things it reads are
# not world-readable, and each has its own module rather than a line here: the
# per-connection table is `0440 root:root` and ./conntrack-reader.nix gives this
# unit's group the right to read it (and turns on the accounting sysctl whose
# counters the table is made of), and dnsmasq's lease file is `0640 root:root` and
# ./lease-reader.nix decides that this unit's group may read it.  It binds
# the LAN address only (`10.3.1.1:8091`); `./firewall.nix` lets services1 reach
# it, and services1's nginx is what serves `network-info.int.leighhack.org`
# (TLS and the ACL stay there) while Prometheus on services1 scrapes `/metrics`.
#
# What that makes visible is worth stating plainly: the per-connection view is a
# list of which device talked to which address, the lease table adds what each of
# those devices calls itself, and both are served to anyone the front door's LAN
# ACL admits (the `int-` rule in `common/config.nix`) — plus services1, which is
# the only remote host `./firewall.nix` opens 8091 to.  The API itself stays
# unauthenticated, as the rest of it always has been; the ACL is the access
# control, and these tables are the reason that sentence now carries more than it
# used to.  `machines/gw/lease-reader.nix` states what handing the lease file's
# names to that audience means, in the same paragraph that decides it.
#
# Consequence for deploying this box: its system now contains a Rust crate, so
# nothing compiles here.  The crate is built on aibox and copied back — gw holds
# `max-jobs = 0`, so a switch started on the box delegates and has no local fallback
# (`machines/gw/default.nix`); `just deploy gw` from anywhere does the same thing over
# ssh.  Do not build it here (AGENTS.md, "Never build on the router").
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  VLANS = import ../../common/vlans.nix;
  TABLES = import ./lib/network-tables.nix;

  # The SPA and the binary that embeds it — `CRANE.spaApp`, the one shape every
  # browser-served app in this fleet shares (common/crane.nix has the recipe, the pinned
  # wasm-bindgen-cli and why the bundle is a build-hook input rather than a cargo
  # dependency; common/frontend-build-spa.sh is the one recipe the flake and
  # `just build-frontend` both run).  This crate is the last one converted to it: its
  # frontend used to be SolidJS, its npm deps were not nixpkgs-cached, so the flake could
  # not rebuild it offline and `dist/` was committed — which is why `--static-dir` and the
  # `pkgs.stdenv.mkDerivation` above it are gone.  What is specific to this app:
  # `sharedCrates` are the ones its manifest reads (`json` for the JSON emitter, `web` for
  # the request/response plumbing and `build-spa` for build.rs), and `ui` is not a Rust
  # dependency of the binary at all — it is a manifest path dependency of the *frontend*
  # (`common-rs/ui` through the git-ignored symlink), so it has to be copied into the
  # frontend's source tree as `spaSharedCrates`, because a symlink cannot travel in a flake
  # source. The tabs are the library's, not written here.
  networkStatus =
    (CRANE.spaApp {
      name = "network-status";
      version = "0.1.0";
      sharedCrates = ["json" "web" "build-spa"];
      spaSharedCrates = ["ui"];
    }).binary;

  # The prefixes the connection view counts as "ours", which is what decides
  # whether a flow is a device downloading something (`out`), something arriving
  # (`in`), LAN-internal (`lan`) or neither (`wan`).  Derived from
  # common/vlans.nix by ./lib/network-tables.nix — the file that already turns
  # those tables into dnsmasq tags and nft sets — rather than repeated here, for
  # the reason that file gives for existing: an address written twice is a fact
  # that can disagree with itself.  To see what the router will be told:
  #
  #   nix-instantiate --eval -E '(import ./machines/gw/lib/network-tables.nix).localPrefixes (import ./common/vlans.nix)'
  #
  # Passed as `--vlan vlan225=10.3.1.0/24` rather than `--local-prefix`: the same
  # prefix either way (both land in one `Localness`, so the direction rules are
  # unchanged), but the labelled form also tells `/api/leases` which VLAN an
  # address was handed out on — one list serving both questions, instead of a
  # second table that could disagree with the one the connection table uses.
  # `./lib/check-network-tables.nix` asserts the two forms describe the same set.
  vlanArgs = builtins.concatLists (map (net: ["--vlan" net]) (TABLES.vlanPrefixes VLANS));

  # The group ./conntrack-reader.nix puts on /proc/net/nf_conntrack, read back out
  # of the option rather than written again here: if that module ever stops being
  # imported, this box fails to evaluate instead of deploying a service that reads
  # nothing and shows a router with no connections.  The account itself is defined
  # there too, with that group as its primary group, because that file is the one
  # that measured why a `DynamicUser` cannot have it (see ./conntrack-reader.nix
  # before changing any of this — the reason is in its header and it is not
  # obvious).
  conntrackGroup = config.users.groups.conntrack-readers.name;
in {
  assertions = [
    {
      # The same fact as above, one step later: `User=` a name that no module
      # creates is a unit that fails at start, on the router, during a switch.
      assertion = config.users.users ? network-status;
      message = ''systemd.services.network-status runs as `User = "network-status"`, which no module defines: ./conntrack-reader.nix (imported by ./default.nix) owns the account and its group.'';
    }
  ];

  systemd.services.network-status = INFRA.mkNeverGiveUp {
    description = "Router network status dashboard (network-info.int.leighhack.org)";
    wantedBy = ["multi-user.target"];
    # Bind the LAN address rather than 0.0.0.0: the only clients are the nginx
    # proxy and Prometheus on services1, so there is no reason to listen on
    # ppp0 or tailscale0 too.  The address comes from networkd, hence the
    # online wait; the never-give-up policy covers a lost race anyway.
    after = ["network.target" "network-online.target"];
    wants = ["network-online.target"];
    # The probe is POSIX sh; the commands it pipes through must be on the unit's
    # PATH rather than inherited from whatever the operator's shell had.
    path = [
      pkgs.bash
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.gawk
    ];

    # The account ./conntrack-reader.nix defines, in serviceConfig because that is
    # where systemd's setting lives.  Not `DynamicUser`: a dynamic uid is given no
    # supplementary groups on this box, and the header of that file has the four
    # measurements that say so.  Without the account the connection table is empty
    # rather than broken — which is why this line sits next to the flags that turn
    # the feature on.
    serviceConfig = {
      Type = "simple";
      User = "network-status";
      # A router dashboard has no business owning a login.  Every file the probe
      # reads is world-readable except the conntrack table, and that one is
      # group-readable to this account's primary group only.
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      NoNewPrivileges = true;
      ExecStart = lib.concatStringsSep " " ([
          "${networkStatus}/bin/network-status"
          "--bind"
          "10.3.1.1"
          "--port"
          "8091"
          "--local"
          "--interval"
          "5"
          "--wan"
          # The AAISP L2TP tunnel (./constants.nix `UPLINK_TUNNEL`), not the hub
          # handoff NIC: ppp0 only exists while the tunnel is up, which is what
          # makes its disappearance the WAN-down signal.
          "ppp0"
          "--title"
          # Quoted: the em dash + spaces would otherwise split the argv.
          "\"Network — router\""
        ]
        # --- the per-connection view (the `/api/connections` table) -------------
        # Byte counters come out of /proc/net/nf_conntrack only while
        # `net.netfilter.nf_conntrack_acct` is on, and count only connections
        # created after it went on, so the sysctl is part of this feature and not a
        # tuning knob — ./conntrack-reader.nix sets it, next to the group read bit.
        ++ [
          "--conn-min-age"
          "5"
          "--conn-max-rows"
          "200"
          "--conn-stale"
          "15"
          # Names for the addresses.  The records file is the flake's own table
          # (world-readable, store-backed, and a `restartTriggers` input to
          # dnsmasq, so it cannot be stale).  The lease file is `0640 root:root` and
          # ./lease-reader.nix is what makes this account able to read it, which is
          # also what makes the `/api/leases` table below have rows at all — an
          # unreadable file is reported as an error by that endpoint, never as an
          # empty table.
          "--hosts-file"
          "/etc/dnsmasq-records.hosts"
          "--leases-file"
          "/var/lib/dnsmasq/dnsmasq.leases"
        ]
        # --- the lease table (`/api/leases`) ------------------------------------
        # dnsmasq stores an expiry and no lease length, so "how much of this lease
        # is left" is recovered from the block the address falls in — and every
        # `dhcp-range` ./lib/network-tables.nix generates is generated at
        # `leaseTtl`.  Read from that table rather than written again here: a
        # `--lease-ttl` that disagreed with dnsmasq's own value would draw every
        # bar the wrong length and nothing else would notice.
        ++ [
          "--lease-ttl"
          (toString TABLES.leaseTtl)
          "--lease-max-rows"
          "500"
        ]
        ++ vlanArgs);
    };
  };
}

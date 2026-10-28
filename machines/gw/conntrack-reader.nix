# MODULE — everything the router needs for `/api/connections` to have data.
#
# `machines/gw/network-status.nix` turns the connection table on and names the
# account it runs as; this file is the other half — the group, the account, the
# accounting sysctl, and the one root step that makes the file readable — because
# all four have to be true before the dashboard's own process starts, and a
# dashboard that can read the table but finds no counters in it is the same empty
# page.
#
#   1. **Byte counters.** `/proc/net/nf_conntrack` prints `packets=`/`bytes=` only
#      while `net.netfilter.nf_conntrack_acct = 1`, so without the sysctl the table
#      holds every connection and none of the numbers the page is for.  It counts
#      only connections created *after* it went on, which is why a router that has
#      just been upgraded shows `bytes: null` for a while; the API calls those
#      `unaccounted` rather than zero, and `router_connections_unaccounted` charts
#      how long "a while" is.
#   2. **Permission to read it.** The file is `0440 root:root`.  It is given the
#      `conntrack-readers` group, and the dashboard's account has that group as its
#      **primary** group.
#
# Both halves were measured on this box on 2026-10-25 with `setpriv`, against the
# real file, and one of the four results killed the design that was here first:
#
#     chgrp render + `render` as PRIMARY group        → reads
#     chgrp render + `render` as SUPPLEMENTARY group  → reads
#     group `nobody`, no supplementary groups         → denied  (the control)
#     `DynamicUser=yes` + `SupplementaryGroups=render` → **denied**, with and
#         without the property: a dynamic uid is given no supplementary groups here
#         (systemd 261), so the connection table is silently empty.
#
# So `DynamicUser` is off for network-status and it has a real system account
# defined below — no login, no home, no shell, `ProtectSystem=strict`,
# `NoNewPrivileges`, and one group.  A previous version of this file paired
# `DynamicUser=yes` with `SupplementaryGroups=` and *claimed in a comment to have
# measured it*; the claim was invented, and the table it produced was empty.
# `CAP_DAC_OVERRIDE`, a world-readable copy, and an ACL by uid (the uid is random)
# are each worse than one account in one group.
#
# **The `chgrp` is an `ExecStartPre=+` on the service that needs it, not a unit of
# its own.**  The first version deployed a oneshot plus a timer to re-apply the
# group; a oneshot whose start fails is enough to fail a `nixos-rebuild switch` on
# the network's router (`Failed to start … - Result: start-limit-hit`), and a
# dashboard with no table is not worth that.  With `+` systemd runs that one line as
# root before the account is dropped into, so it happens on every start and every
# restart, needs no ordering, and adds nothing that can block an activation.
#
# What that gives up, stated plainly: reloading `nf_conntrack` mid-run re-creates the
# file `root:root` and the dashboard goes blind until it restarts.  It says so
# rather than showing an empty router (`network-status: the conntrack table is not
# being read at all`, with the file's owner/group/mode), and reloading that module
# would take NAT with it.  If it ever is a real problem the self-healing shape to
# copy is `common/nixbld.nix`'s guard timer — but that module ships a timer for a
# thing an operator genuinely does by hand, which is the test this one failed.
#
# What this deliberately does **not** do is make the table readable by anyone else.
# It is a list of every device on the network and who it is talking to, so the group
# has exactly one member: the dashboard's own `network-status` account, which no human
# logs in as.  That one file in `/var/lib/dnsmasq` used to be the counter-example here —
# the lease file holds the *names* of DHCP clients, and handing it to this group was a
# decision nobody had made.  It has been made: `./lease-reader.nix` is that module, it
# puts the lease file in *this* group for the same single member, and its header is where
# the reasoning lives.  Adding a second member to `conntrack-readers` now grants both —
# which is the reason to read that header rather than add a line.
{pkgs, ...}: let
  conntrackGroup = "conntrack-readers";
  conntrackFile = "/proc/net/nf_conntrack";

  # Idempotent, and quiet when there is nothing to do: the journal is not a
  # heartbeat.  `modprobe` first because `nf_conntrack` is a *module* on this box
  # (`lsmod`, held up by nft_ct/nft_nat) and a box that comes up without it has no
  # file to fix — exiting 0 there is right, because nothing is broken and the
  # dashboard's own probe says out loud when the table is missing.
  grant = pkgs.writeShellScript "conntrack-reader" ''
    set -eu
    f=${conntrackFile}
    ${pkgs.kmod}/bin/modprobe nf_conntrack 2>/dev/null || true
    [ -e "$f" ] || exit 0
    ${pkgs.procps}/bin/sysctl -w net.netfilter.nf_conntrack_acct=1 >/dev/null 2>&1 || true
    if [ "$(stat -c '%G' "$f")" = "${conntrackGroup}" ]; then
      exit 0
    fi
    echo "${conntrackFile}: was $(stat -c '%U:%G %a' "$f"), granting ${conntrackGroup}" >&2
    ${pkgs.coreutils}/bin/chgrp ${conntrackGroup} "$f"
    ${pkgs.coreutils}/bin/chmod g+r "$f"
  '';
in {
  # Both halves of the account live here rather than in ./network-status.nix, so
  # that the file which says *why* a `DynamicUser` cannot read this is also the file
  # that says what reads it instead.  The name is the unit's; ./network-status.nix
  # sets `User = "network-status"` and an assertion there refuses an evaluation
  # where the account has gone missing.
  users.groups.${conntrackGroup} = {name = conntrackGroup;};
  users.users.network-status = {
    isSystemUser = true;
    group = conntrackGroup;
    description = "network-status dashboard (network-info.int.leighhack.org)";
  };

  # Accounting as a boot sysctl, so it is on before anything holds a connection
  # worth counting; the same value is re-asserted by `grant` because that is
  # cheaper than arguing about which unit wins at boot.
  boot.kernel.sysctl = {
    "net.netfilter.nf_conntrack_acct" = 1;
  };

  # `mkBefore` so a caller that adds its own ExecStartPre cannot push the
  # permission step after the read it is there to enable.
  systemd.services.network-status.serviceConfig.ExecStartPre = pkgs.lib.mkBefore [
    "+${grant}"
  ];
}

# MODULE — who may read dnsmasq's lease file, and how it stays readable.
#
# `/var/lib/dnsmasq/dnsmasq.leases` is the router's record of which client holds
# which address, and it is `0640 root:root`.  `./network-status.nix` serves a lease
# table out of it (`/api/leases`), so it needs to be able to open it — and that is
# a decision, not a side effect: this file used to say the decision had *not* been
# made, and the reason it can be made now is written below.
#
# ## What the decision is
#
# The file is handed to the group the dashboard's account already has
# (`conntrack-readers`, defined in ./conntrack-reader.nix), keeping mode 0640.  So:
#
#   * it is **not** world-readable, and it is not readable by any account the LAN
#     can reach — the group has exactly one member, `network-status`, which is a
#     `ProtectSystem=strict`, `NoNewPrivileges` system account with no login;
#   * what that member can see is what the page already shows: a device's
#     hostname, its MAC address, its client-id and the address it was given.  The
#     conntrack table is *already* the list of which device talked to where, so
#     this adds names to rows that exist rather than a new class of fact about the
#     network;
#   * it stays inside the box.  `/api/leases` is unauthenticated like the rest of
#     the API, because the access control is the front door's LAN ACL
#     (`common/config.nix`, `int-` rule) — the same sentence
#     `./network-status.nix` already carries for the connection table, and the
#     same audience.
#
# The group is named for conntrack because it was created for it; renaming it
# would be a second group with one member and a migration for a word.  What the
# name actually means is "the dashboard's read-only inputs", and that is what this
# file and ./conntrack-reader.nix both put in it.
#
# ## Why a path unit and not just a tmpfiles rule
#
# The mode has to survive dnsmasq.  dnsmasq owns the file and rewrites it whenever
# a lease is taken, renewed or expires — by truncating it in place as far as this
# box can tell (the file is still 0640 with 49 leases in it, which a
# create-with-umask would not be), but "as far as this box can tell" is not
# something to build a permission on: a dnsmasq upgrade that switches to
# write-temp-then-rename would silently hand the file back to `root:root` and the
# page would go back to showing numbers, on a box whose lease table is how you
# find a device.
#
# So the group is applied in three places, each of which is cheap and idempotent
# and none of which is a timer running for no reason:
#
#   1. a tmpfiles rule, so the file has the group from the moment it is created
#      (`f`, then `z` — `f` creates, `z` adjusts an existing file, and the order
#      inside one module's list is preserved);
#   2. `lease-reader.service`, a oneshot that says nothing and does nothing when
#      the group is already right — started by `lease-reader.path`, which watches
#      `/var/lib/dnsmasq` for an entry being created, removed or replaced.  That
#      is exactly the case that would lose the group: an in-place rewrite keeps the
#      inode, and with it the group and the mode, which is what the live file shows
#      (measured 2026-10-28 on the box: overwritten as `root:root 600`, back to
#      `root:conntrack-readers 640` within a second, and the content byte-identical);
#   3. an `ExecStartPre=+` on network-status, so a start never races the path unit
#      and the first `/api/leases` after a boot is not a 503.
#
# The path unit is the one that differs from ./conntrack-reader.nix, and the
# difference is the point: `/proc/net/nf_conntrack` only loses its group when
# someone reloads a kernel module, which no routine operation does, whereas
# dnsmasq replaces its lease file often enough that a permission which survived
# only until the next replacement would be a permission that quietly stopped
# working.  A watch on a directory that changes a few times an hour is not the
# timer-that-polls shape that file argues against.
#
# ## What this does not do
#
# It does not make the file readable by anyone else, it does not copy it
# somewhere world-readable, and it does not give the service `CAP_DAC_OVERRIDE`.
# An unreadable file is reported by `/api/leases` as an error naming the OS's own
# reason, never as an empty table — because "no DHCP clients" is a conclusion an
# operator would act on, and it would be wrong.
{
  config,
  lib,
  pkgs,
  ...
}: let
  # Read out of the option rather than spelled again: this module adds permissions
  # for a group another module owns, and a hard-coded name here would be a second
  # definition of it that could drift into a unit that cannot read anything.
  group = config.users.groups.conntrack-readers.name;
  leaseFile = "/var/lib/dnsmasq/dnsmasq.leases";

  # Idempotent and quiet when there is nothing to do: the journal is not a
  # heartbeat, and this runs on every lease rewrite.
  grant = pkgs.writeShellScript "lease-reader" ''
    set -eu
    f=${leaseFile}
    [ -e "$f" ] || exit 0
    if [ "$(stat -c '%G %a' "$f")" = "${group} 640" ]; then
      exit 0
    fi
    echo "$f: was $(stat -c '%U:%G %a' "$f"), granting ${group} read" >&2
    ${pkgs.coreutils}/bin/chgrp ${group} "$f"
    ${pkgs.coreutils}/bin/chmod 0640 "$f"
  '';
in {
  assertions = [
    {
      # The group comes from ./conntrack-reader.nix.  If that module stops being
      # imported, a tmpfiles rule naming a group nobody creates fails an activation
      # on the network's router — better to refuse the evaluation and say why.
      assertion = config.users.groups ? ${group};
      message = ''lease-reader.nix puts the lease file in group `${group}`, which no module defines: ./conntrack-reader.nix owns it (imported by ./default.nix).'';
    }
  ];

  systemd.tmpfiles.rules = [
    # `f` creates it (dnsmasq needs the file to exist, and a lease file it cannot
    # open is a lease file it replaces on its own terms); `z` adjusts the group of
    # one that already exists.  Both in this module, in this order, so the two
    # halves cannot be separated by a module-ordering accident.
    "f ${leaseFile} 0640 root ${group} -"
    "z ${leaseFile} 0640 root ${group} -"
  ];

  systemd.services.lease-reader = {
    description = "Make dnsmasq's lease file readable by the dashboard";
    # No `wantedBy`: the path unit starts it, and the ExecStartPre below runs the
    # same script.  A service that is also wanted by multi-user.target would be a
    # second thing that can fail a switch for no extra benefit.
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "+${grant}";
      # Reading a stat and maybe one chgrp: nothing here should be able to hang,
      # and a watchdog on a oneshot is how a wedged activation is noticed early.
      TimeoutStartSec = "30s";
    };
  };

  # Watch the directory rather than the file: a rename replaces the inode, and a
  # path unit pointed at the old inode would keep watching a file nobody writes.
  systemd.paths.lease-reader = {
    description = "Watch dnsmasq's lease directory for a rewrite";
    wantedBy = ["multi-user.target"];
    pathConfig = {
      PathChanged = "/var/lib/dnsmasq";
      Unit = "lease-reader.service";
    };
  };

  # Before the dashboard's own process, so a boot does not serve a 503 for the
  # lease table while it waits for the path unit.
  systemd.services.network-status.serviceConfig.ExecStartPre = lib.mkBefore [
    "+${grant}"
  ];
}

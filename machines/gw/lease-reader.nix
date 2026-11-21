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
# The mode has to survive dnsmasq, because dnsmasq takes the file back.  It owns
# the file, rewrites it whenever a lease is taken, renewed or expires, and takes it
# back **on ownership every time it starts**: restarting dnsmasq on this box at
# 2026-10-31 14:28:35 produced
#
#     /var/lib/dnsmasq/dnsmasq.leases: was root:root 640, granting conntrack-readers read
#
# from the re-grant script below, on the **same inode** the file had before the
# restart.  Two facts, and getting them the right way round is what the watch at
# the bottom of this file is for:
#
#   * the rewrite is **in place** — truncate-and-write on the descriptor the daemon
#     opened when it started.  The inode has not changed once in this file's life,
#     so a rewrite keeps the group and the mode and needs no fix;
#   * what *does* lose the group is **dnsmasq starting**, which chowns the file
#     `root:root`.  Rename-free, inode-preserving, and it happens on every
#     activation that restarts dnsmasq — which is every DNS-table change, because
#     the network tables are dnsmasq `restartTriggers`.
#
# So the group is applied in four places, each of which is cheap and idempotent
# and none of which is a timer running for no reason:
#
#   1. a tmpfiles rule, so the file has the group from the moment it is created
#      (`f`, then `z` — `f` creates and does not truncate, `z` adjusts an existing
#      file, and the order inside one module's list is preserved);
#   2. `lease-reader.service`, a oneshot that says nothing and does nothing when
#      the group is already right — started by `lease-reader.path`, which watches
#      `/var/lib/dnsmasq` for an entry being created, removed or replaced **or
#      chowned** (an inotify attribute event on a child does reach a watch on its
#      directory).  An in-place rewrite keeps the inode, and with it the group and
#      the mode, which is what the live file shows (measured 2026-10-28 on the box:
#      overwritten as `root:root 600`, back to `root:conntrack-readers 640` within a
#      second, and the content byte-identical) — but dnsmasq's own start *is* an
#      attribute change on that inode, and it does fire this;
#   3. an `ExecStartPre=+` on network-status, so a start never races the path unit
#      and the first `/api/leases` after a boot is not a 503;
#   4. an `ExecStartPost=+-` on **dnsmasq**, because a path unit's trigger is an
#      event and an event can be lost.  Measured 2026-11-21: `systemctl restart
#      dnsmasq` at 11:31:08 (the prescribed repair for the dead-RA-timer fault,
#      `machines/gw/README.md`) fired the path unit twice, both runs finished while
#      the group was still right, dnsmasq's chown landed after them, and its own
#      event was merged into a start job that was already in flight — so no third
#      run followed and the file was `root:root 0640` again.  The page then answered
#      503 with `/var/lib/dnsmasq/dnsmasq.leases: Permission denied (os error 13)`
#      for three and a half hours.  A `chgrp root` on the same box afterwards, with
#      no run in flight, was repaired inside a second — which is what makes the
#      first case a lost event rather than a broken watch.  Running the same
#      idempotent grant once dnsmasq has started asks for no event at all; `+-`
#      because a chgrp that fails must not fail the daemon that answers DNS and DHCP
#      for the building (the watch below is the alarm for that).
#
# The path unit is the one that differs from ./conntrack-reader.nix, and the
# difference is the point: `/proc/net/nf_conntrack` only loses its group when
# someone reloads a kernel module, which no routine operation does, whereas this
# file loses its group every time dnsmasq starts.  A watch on a directory that
# changes a few times an hour is not the timer-that-polls shape that file argues
# against.
#
# ## What froze the page for two and a half days, and what watches it now
#
# Nothing in the flake caused this, and nothing in dnsmasq did.  The sudo log says
# what did, at 2026-10-28 20:51:20:
#
#     cp -p     /var/lib/dnsmasq/dnsmasq.leases /tmp/leases.bak
#     install -m 0600 -o root -g root /tmp/leases.bak /var/lib/dnsmasq/dnsmasq.leases
#
# a debugging session's own backup-and-restore.  `install` unlinks before it
# writes, so the path became a **new inode** while dnsmasq went on writing the old
# one — the mechanism above read backwards: an in-place writer is precisely a writer
# you may not replace from under.  For the next 2 d 17 h the page served that new
# inode: 56 leases, every one already lapsed, and no `10.3.2.212` — which dnsmasq
# ACKed eight times in the interval, and which was the device the session was
# looking for.  `/api/leases` said `"expired":56` throughout, which was true and
# was not enough: a table of nothing-but-lapsed rows reads as "nobody is renewing",
# not as "nobody is writing".
#
# Three things follow, and all three are in place now.
#
#   1. **The page says so.**  `crates/network-status/src/leases.rs` stats the file
#      as well as reading it and `/api/leases` answers `stale` — `"all-expired"`
#      when every row has lapsed (a serving DHCP server always has one that has
#      not), `"unwritten"` when the file is older than two lease lengths (dnsmasq
#      rewrites it on every lease event) — and the leases panel banners it.  That
#      verdict is the *only* definition of stale in the tree: the watch below reads
#      it over the API rather than keeping a second one in shell.
#   2. **`lease-reader-watch.service`**, on a 5-minute timer, fails loudly when the
#      channel dies and does nothing else.  It will not restart dnsmasq: this is
#      the box that answers DNS and DHCP for the building, and a stale lease file
#      is not worth interrupting either of them for on a timer.  A red unit is the
#      signal — `systemctl --failed`, and the same failed-unit metric every other
#      check on this box alerts through.  Its message names the repair.
#   3. **The live database is never lost**, because dnsmasq holds it open: root can
#      read it at `/proc/$(pgrep -x dnsmasq)/fd/3` even when the link reads
#      `(deleted)` and the path has not moved in days.  That is how this was
#      repaired, and it is the repair for any recurrence — in this order, because
#      the file has to hold the daemon's own current state *before* the restart
#      that loads it, or every client whose lease lives only in memory is offered
#      something else:
#
#          cat /proc/$(pgrep -x dnsmasq)/fd/3 | sudo tee /var/lib/dnsmasq/dnsmasq.leases >/dev/null
#          sudo systemctl restart dnsmasq              # reopens the path and loads what is on it
#
#      then verify: `fd/3` has no `(deleted)`, the mtime is now, the re-grant unit
#      logged (dnsmasq's own start chowns the file, and ./lease-reader.path hands
#      the group back inside a second), and `/api/leases` answers `"stale":null`
#      with the client in question in the table.  `tee`/`cat >` truncate in place —
#      same inode, same owner, same group — which is the entire point: `install`,
#      `mv`, `rsync` without `--inplace` and an editor's save-replace all unlink
#      first, and that is the accident being avoided.  And `SIGHUP` is *not* a
#      repair: measured the same day, `systemctl kill -s HUP dnsmasq` left the fd
#      `(deleted)` and the mtime unmoved, so a restart is the only way back, which
#      is cheap precisely because the database was written back out first.
#
# ## What this does not do
#
# It does not make the file readable by anyone else, it does not copy it
# somewhere world-readable, and it does not give the service `CAP_DAC_OVERRIDE`.
# The watch **restarts nothing** — that is the line, because this is the box that
# answers DNS and DHCP for the building and no timer is allowed to interrupt them
# — and it never writes the lease file's *contents*; what it does do is re-grant
# the file's group when it finds that lost, because a permission is a fact it can
# read and assert where an inotify event may simply never arrive (2026-11-21 in
# this file's header).  An unreadable file is reported by `/api/leases` as an error
# naming the OS's own reason, never as an empty table — because "no DHCP clients" is
# a conclusion an operator would act on, and it would be wrong.
#
# ## The alarm that reaches a person is not on this box
#
# A failed unit is only visible to someone running `systemctl --failed`, and this
# fleet's Prometheus `ServiceFailed` rule has **no receiver** (`machines/services1/
# services/observability.nix`: "No alertmanager is wired up yet").  That is why the
# same condition is also a gatus endpoint — `Router lease table (network-info)` in
# `common/monitoring-endpoints.nix`, which goes to Slack like every other check on
# the board — and why it is an HTTP check on `/api/leases` rather than on the page:
# a dashboard can answer 200 while the file it reads is unreadable.  That row also
# requires `"stale":null` in the body, because the other way this channel dies (the
# file replaced under dnsmasq's descriptor, 2026-10-28) answers **200** and only the
# verdict says so.
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

  # Where the dashboard reports, read back out of the unit that serves it rather
  # than written down a second time: network-status binds this box's **LAN**
  # address rather than loopback (`./network-status.nix` explains why — the probe
  # is `--local`), so "localhost and the usual port" would be a second answer to
  # "where is this service" and a wrong one.  A match that fails is an evaluation
  # error, which is the right moment to notice the unit changed shape; the gate
  # that runs it is `just check-eval`.
  apiEndpoint = let
    v = config.systemd.services.network-status.serviceConfig.ExecStart;
    execLine =
      if lib.isList v
      then lib.concatStringsSep " " (map toString v)
      else toString v;
    m = builtins.match ''.*--bind +([^ ]+) +--port +([0-9]+).*'' execLine;
  in
    if m == null
    then throw "lease-reader.nix: no `--bind … --port …` in network-status's ExecStart, so the lease-channel watch has no address to ask: ${execLine}"
    else "http://${lib.elemAt m 0}:${lib.elemAt m 1}";

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
  # The channel, checked — and, in the one case it can fix, repaired.  Three steps,
  # deliberately of different severity:
  #
  #   0. the grant, **read rather than waited for**.  Its trigger is an inotify event
  #      and an event can be lost (the 2026-11-21 restart in this file's header), so
  #      the state it maintains is also asserted here.  A lost event then costs the
  #      page at most this timer's interval instead of three and a half hours, and
  #      the check still fails — it repaired something the event-driven path missed;
  #   1. the dashboard's own verdict, taken from `/api/leases` so that this file
  #      holds no second definition of "stale" to drift from the one the page
  #      banners on (`crates/network-status/src/leases.rs::stale`);
  #   2. the mechanism, which only root can see: whether the file dnsmasq holds
  #      open for writing is the file on disk.
  watch = pkgs.writeShellScript "lease-reader-watch" ''
    set -eu
    lease=${leaseFile}
    fail() { echo "lease channel: $*" >&2; exit 1; }

    [ -e "$lease" ] || fail "${leaseFile} does not exist at all — dnsmasq creates it when it starts, so this is dnsmasq's fault rather than a permissions one"

    # 0. Re-grant, and say so.  Idempotent, silent when the group is already right,
    #    and it touches nothing but the file's group and mode — the same chgrp
    #    `lease-reader.service` performs at boot and dnsmasq's `ExecStartPost`
    #    performs after every start.  This is not the "witness, not an actor" rule
    #    above being broken: that rule is about never *restarting* dnsmasq from a
    #    timer, and this restarts nothing.  The check still exits non-zero when it
    #    had to repair, because a repair means the trigger that should have done it
    #    was missed — a red unit (or the gatus row beside this in
    #    `common/monitoring-endpoints.nix`) is how that stays visible.
    repaired=""
    if [ "$(${pkgs.coreutils}/bin/stat -c '%G %a' "$lease")" != "${group} 640" ]; then
      repaired=$(${pkgs.coreutils}/bin/stat -c '%U:%G %a' "$lease")
      ${pkgs.coreutils}/bin/chgrp ${group} "$lease"
      ${pkgs.coreutils}/bin/chmod 0640 "$lease"
      echo "lease channel: ${leaseFile} was $repaired — re-granted ${group} 0640 (the grant's trigger was missed; machines/gw/lease-reader.nix)" >&2
    fi

    # 1. What the page believes.  Not run when the dashboard is not up: its
    #    absence is a different fault with its own signal, and failing here too
    #    would be two alarms for one cause.
    if ${pkgs.systemd}/bin/systemctl is-active --quiet network-status; then
      # No `curl -f` here, deliberately: `-f` throws the *body* away along with the
      # status, and an unreadable lease file is a 503 whose body is the sentence
      # that names the fault (`{"error":"…: Permission denied (os error 13)"}`).
      # Until 2026-11-21 this step used `-f`, so a broken channel arrived here as an
      # empty string and was reported as a "note" — the page answered 503 for three
      # and a half hours while this timer said nothing about it.
      body=$(${pkgs.coreutils}/bin/mktemp)
      trap 'rm -f "$body"' EXIT
      code=$(${pkgs.curl}/bin/curl -sS -m 5 -o "$body" -w '%{http_code}' ${apiEndpoint}/api/leases 2>/dev/null || true)
      case "$code" in
        200) : ;;
        "") fail "network-status is up but ${apiEndpoint}/api/leases did not answer at all" ;;
        *) fail "network-status cannot read the lease file — HTTP $code from ${apiEndpoint}/api/leases: $(${pkgs.coreutils}/bin/head -c 300 "$body")" ;;
      esac
      case "$(cat "$body")" in
        *'"stale":null'*) : ;;
        *)
          verdict=$(${pkgs.gnused}/bin/sed -n 's/.*"stale":\("[a-z-]*"\).*/\1/p' "$body")
          [ -n "$verdict" ] || verdict='(no verdict in the response)'
          fail "the dashboard's own verdict on ${leaseFile} is $verdict — ${apiEndpoint}/api/leases"
          ;;
      esac
    fi

    # 2. The mechanism.  dnsmasq keeps this fd open for its whole life and
    #    truncates it in place, so a target of `<path> (deleted)` means every
    #    write since has landed on an inode no reader can reach — exactly the
    #    2026-10-28 shape.  Only that is a failure: a future dnsmasq that closed
    #    the file after loading it would be a version note rather than an outage,
    #    and 1 catches the outage either way.
    held=0
    for pid in $(${pkgs.procps}/bin/pgrep -x dnsmasq || true); do
      for fd in /proc/$pid/fd/*; do
        t=$(${pkgs.coreutils}/bin/readlink "$fd" 2>/dev/null || true)
        case "$t" in
          "$lease (deleted)")
            fail "dnsmasq (pid $pid) is writing a deleted inode.  Restore its live database in place and restart it: cat /proc/$pid/fd/* | tee $lease >/dev/null; systemctl restart dnsmasq"
            ;;
          "$lease") held=1 ;;
        esac
      done
    done
    [ "$held" = 1 ] || echo "lease channel: note — no dnsmasq fd on $lease" >&2

    # Last, so everything above has still been checked: the repair is a failure to
    # report, not a reason to skip the rest.
    [ -z "$repaired" ] || fail "${leaseFile} was $repaired and this check re-granted ${group} 0640 — the page answered 503 until now. Nothing else is wrong; the event-driven grant was missed (machines/gw/lease-reader.nix, 2026-11-21)."
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
    {
      # `apiEndpoint` is lifted out of that unit's own command line, so a box that
      # stops defining the service has to fail here rather than on a string match
      # against an option nothing set.
      assertion = config.systemd.services ? network-status;
      message = ''lease-reader.nix asks network-status where the lease API answers, and ./network-status.nix (imported by ./default.nix) does not define that unit.'';
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
    #
    # Waits for dnsmasq, because the event that triggered it is usually dnsmasq's
    # own start: an in-place rewrite of the lease file does not touch the group, and
    # the chown does — so a run that starts *before* the daemon is up is a run that
    # reads a group nobody will break for another few milliseconds (the 2026-11-21
    # loss in this file's header).  Ordering it after the daemon means a
    # path-triggered run cannot finish before the chown it exists to undo.
    after = ["dnsmasq.service"];
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

  # And after every dnsmasq start, which is the one thing that takes the group back.
  # `-` because a chgrp that fails must not fail the daemon answering DNS and DHCP
  # for the building; `lease-reader-watch` is the alarm for that, and it is red
  # within five minutes.  This is the half that does not depend on an inotify event
  # surviving — see this file's header for the restart that lost one.
  systemd.services.dnsmasq.serviceConfig.ExecStartPost = lib.mkAfter [
    "+-${grant}"
  ];

  systemd.services.lease-reader-watch = {
    description = "Check that the DHCP lease page's source is still being written";
    # No wantedBy: the timer starts it, and a service also pulled into
    # multi-user.target would be a second way to fail a switch for no coverage.
    # It is worth starting by hand once (`systemctl start lease-reader-watch`),
    # because a check that has never run is a check whose message nobody has read.
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "+${watch}";
      TimeoutStartSec = "30s";
    };
  };

  systemd.timers.lease-reader-watch = {
    description = "Every 5 minutes, check the lease page's source is live";
    wantedBy = ["timers.target"];
    timerConfig = {
      # The house shape: once after boot, then every interval, so a box that
      # comes up broken says so without waiting for an activation.
      OnBootSec = "5min";
      OnUnitActiveSec = "5min";
    };
  };
}

# MODULE — the network's two paper printers, as CUPS queues on this image.
#
# The Pi Room's PCs are the room's workstations: LibreOffice, Chrome and the office
# suite are on this image (machines/pi-room-sys/configuration.nix), and until 2026-11-21
# **nothing on this image could print at all** — `services.printing` appears nowhere in
# the flake, so a Pi Room user holding a document had no queue to print it into. This
# module is the whole plumbing: CUPS on the image, one queue per printer, and the
# address read out of the router's table rather than written here.
#
# The DATA (how each device is dialled, and what proved it) is ./printers-data.nix;
# docs/paper-printers.md owns the protocol receipts, the proven/guessed split and the
# open items. Read that before changing a row: every value there is marked with the
# measurement that put it there or with the word GUESS.
#
# Three things make this worth its own file rather than lines in configuration.nix, and
# all three are the reason the assertions below exist:
#
# * The address of a printer is common/dns-records.nix's fact, and an image that
#   spelled its own copy of it would keep building after the record moved — printing
#   into a queue that dials a dead address, which is invisible until someone tries to
#   print. So a row names its *record* and ./printers-data.nix assembles the URI from
#   it; the assertion here is what refuses the image when the record is gone, and
#   machines/gw/lib/check-network-tables.nix carries the same rule on the eval-only
#   side, where it prints the record's name on the router.
# * This image has no disk: root is tmpfs and `/var/lib/cups` — where `printers.conf`
#   and the queues themselves live — is therefore re-provisioned on every boot, and
#   nothing persists across one. That is why startWhenNeeded is forced off below: left
#   at the module's default, CUPS is socket-activated, so the queues get registered
#   *when the first connection arrives* — i.e. inside the first print job rather than
#   before it — and on an image nobody ever boots twice in a row, "wait for a reboot to
#   pick up a changed printer" is wait forever.
#
# * **The queues are registered by a unit of their own, not by `cups.service`'s
#   post-start, and that is a measured correction (2026-11-21, on a booted client at
#   10.3.14.204).** `hardware.printers.ensurePrinters` generates an ExecStartPost for
#   `cups.service` that runs with `set -e`: one `lpadmin` that cannot register its
#   queue kills the *scheduler*, so the box has no printers at all rather than one
#   fewer — and systemd's default start rate limit then leaves CUPS permanently
#   failed. Both halves happened that day, for two different reasons: the `brother`
#   row carried `net+ippt://`, which CUPS 2.4.19 refuses outright (`lpadmin: Bad
#   device-uri scheme "net+ippt"`), and `-m everywhere` is *driverless*, i.e. it asks
#   the device for its attributes at registration, so a printer that is switched off
#   when the client boots fails the same way (`Unable to connect … Host is down`).
#   The unit below retries each row independently and leaves the scheduler alone, so
#   a sleeping printer costs its own queue and nothing else. docs/paper-printers.md
#   owns the receipts.
#
# * **A printer that is off still gets a queue, and that is the second correction of
#   2026-11-21.** Retrying alone was not enough: a room's desk with no destination in
#   the dialog reads as "printing is broken", and both printers were off when the next
#   report came in. So a row that cannot go driverless gets a *placeholder* queue —
#   `lpadmin` without `-m`, which returns 0 against a dark device — held with
#   `cupsreject` and one sentence of reason, and upgraded to the driverless PPD the
#   moment the device answers TCP. The state machine, and the measured trap it is built
#   around (a failed `-m everywhere` *deletes* an existing queue), are in the script's
#   own comment below.
#
# Every definition below carries its `config.` prefix on purpose: this module's
# guardrails are the list under `config.assertions`, and nixpkgs' module machinery
# reports a module that mixes bare option names with that list as
# "has an unsupported attribute `hardware`" — an error that names the wrong half and
# points at nothing, which is why the prefix is stated rather than inherited.
{
  config,
  lib,
  pkgs,
  ...
}: let
  RECORDS = import ../../common/dns-records.nix;
  PRINTERS = import ./printers-data.nix;

  queues = builtins.attrNames PRINTERS;

  # Substring test without regex — same helper as common/check-printer-states.nix and
  # machines/gw/lib/check-network-tables.nix, same reason: the needles are full of
  # regex metacharacters (they are addresses and paths).
  has = sub: s: builtins.replaceStrings [sub] [""] s != s;

  # A row whose record is gone.  Reported rather than thrown, so the finding names the
  # row and the record — reading `RECORDS.${x}.ipv4` on a missing name dies in the
  # evaluator with this file's path instead, which is the class of unhelpful failure
  # machines/gw/lib/check-network-tables.nix's history is about.
  missingRecord = n: (RECORDS.${PRINTERS.${n}.record} or null) == null;
  badRecords = builtins.filter missingRecord queues;

  defaults = builtins.filter (n: PRINTERS.${n}.default) queues;

  # The address a row dials, read out of the record it names — the same expression
  # ./printers-data.nix builds the URI from, so the probe below cannot dial an address
  # the queue does not.  A missing record falls back rather than throwing here for the
  # reason that file states; the assertion names it.
  addrOf = n: (RECORDS.${PRINTERS.${n}.record} or {ipv4 = "0.0.0.0";}).ipv4;

  # The reason a placeholder queue is held with, shown in the app's print dialog and in
  # the queue's own state.  It has to say what the *user* can do about it, because the
  # alternative — no destination at all — is what sent someone to look for a bug here.
  rejectReason = "the printer is switched off or asleep; this queue accepts jobs when it answers";

  # One call per row, all of them running the function below with that row's four
  # values.  Keeping the logic in one `provision_one` rather than generating a block per
  # row is deliberate: this is a state machine (absent → placeholder → driverless), and
  # two copies of a state machine is how they stop agreeing.
  rowCall = n: let
    row = PRINTERS.${n};
  in ''
    provision_one ${lib.escapeShellArg n} ${lib.escapeShellArg row.deviceUri} \
      ${lib.escapeShellArg (addrOf n)} ${lib.escapeShellArg (builtins.toString row.port)} \
      ${lib.escapeShellArg row.description}
  '';

  provision = pkgs.writeShellScript "paper-printers-provision" ''
    set -u
    # A loop rather than a one-shot: this image registers its queues at every boot, so
    # the pass that runs before a printer is switched on has to be repeatable — and it
    # has to keep running after every row exists, because "exists" is not the same as
    # "is driverless" (see the placeholder branch).  It exits 0 only when every row has
    # a driverless queue, which is the state a healthy boot ends in.
    #
    # Why a placeholder exists at all (measured on a booted client 2026-11-21):
    # `-m everywhere` asks the *device* for its attributes, so a printer that is switched
    # off gets no queue, and a print dialog with no destinations over two devices that are
    # physically there reads as "printing is broken" rather than "the printer is off" —
    # that is exactly the report this fixes.  `lpadmin -p X -E -v <uri>` with no `-m` or
    # `-i` creates a driverless-less queue and returns 0 against a device that is dark
    # (measured: rc=0, `printer X is idle. enabled`), so the destination can exist; holding
    # it with `cupsreject` means a job is refused with the sentence below instead of
    # spooling into a PPD-less queue and printing whatever arrives.  When the printer
    # answers, the same call that registers a queue upgrades the placeholder.  A failed
    # `-m everywhere` **against an existing queue deletes it** (measured, same day: the
    # destination was gone afterwards), which is why every failure path re-creates the
    # placeholder instead of assuming the old one survived.
    provision_one() {
      name=$1 uri=$2 addr=$3 port=$4 desc=$5
      state=/run/paper-printers/$name

      # The device answers TCP on its IPP port.  A probe rather than trusting `lpadmin`
      # to fail cheaply: a failed driverless attempt on an existing queue removes it.
      reachable() {
        ${pkgs.coreutils}/bin/timeout 3 ${pkgs.bash}/bin/bash -c "exec 3<>/dev/tcp/$addr/$port" 2>/dev/null
      }

      placeholder() {
        if ${pkgs.cups}/bin/lpadmin -p "$name" -D "$desc" -E -v "$uri" >/dev/null 2>&1; then
          ${pkgs.cups}/bin/cupsreject -r ${lib.escapeShellArg rejectReason} "$name" >/dev/null 2>&1 || true
          echo placeholder >"$state"
        fi
      }

      if ${pkgs.cups}/bin/lpstat -p "$name" >/dev/null 2>&1 &&
        [ "$(cat "$state" 2>/dev/null)" = driverless ]; then
        return 0
      fi

      if reachable &&
        ${pkgs.cups}/bin/lpadmin -p "$name" -D "$desc" -E -m everywhere -v "$uri" >/dev/null 2>&1; then
        echo driverless >"$state"
        ${pkgs.cups}/bin/cupsaccept "$name" >/dev/null 2>&1 || true
        echo "paper printers: $name registered (driverless)"
        return 0
      fi

      # A placeholder, created once and refreshed only if it vanished — re-running
      # `lpadmin` every 20 s would churn the queue (and its
      # `enabled since` state) for no reason, and a failed driverless attempt above is
      # the one way the destination can disappear.
      if ${pkgs.cups}/bin/lpstat -p "$name" >/dev/null 2>&1 &&
        [ "$(cat "$state" 2>/dev/null)" = placeholder ]; then
        : # already held — re-running lpadmin every 20 s would only churn the queue
      else
        placeholder
        echo "paper printers: $name is not answering — queue held with a reason until it does" >&2
      fi
      pending="''${pending} $name"
    }

    while :; do
      pending=""
      ${lib.concatMapStringsSep "\n" rowCall queues}
      if [ -z "$pending" ]; then
        ${
      lib.optionalString (defaults != []) ''
        # A default is a decision, not a fact, so the table owns it and `null` — no
        # default, the app asks — is the legal answer this image gives today:
        # nothing measured says which of the two printers a Pi Room desk should get
        # when nobody names one, and picking one on a hunch is a document leaving
        # for the wrong machine.
        ${pkgs.cups}/bin/lpadmin -d ${lib.escapeShellArg (builtins.head defaults)} || true
      ''
    }
        exit 0
      fi
      echo "paper printers: still not driverless:$pending — retrying in 20 s" >&2
      sleep 20
    done
  '';
in {
  config.services.printing.enable = true;

  # Never socket-activated, for the reason in the header.  The module's own `drivers`
  # list stays empty on purpose: the vendors' PPDs are not in nixpkgs, and
  # `model = everywhere` (./printers-data.nix) asks the device instead of needing a
  # driver, so `cups-filters` + ghostscript — which the module adds unconditionally —
  # is the whole filter chain this flake can build.
  config.services.printing.startWhenNeeded = false;

  # `partOf` as well as `requires`: a `cups` restart has to re-run this, because the
  # queues it registered are `cups`' own state; `requires` alone would only stop it
  # when the scheduler stops, which is not the same thing.  The unit's own script
  # carries the device URIs, so an edited row changes it and a switch restarts it —
  # which is what the old `restartTriggers` on `cups.service` was for.
  config.systemd.services.paper-printers = {
    description = "Register the Pi Room's paper printers with CUPS";
    after = ["cups.service"];
    requires = ["cups.service"];
    partOf = ["cups.service"];
    wantedBy = ["multi-user.target"];
    # The retry loop is the script's own, so the unit's start timeout must not end it;
    # and a printer that is off for a week is not a start failure to rate-limit.
    startLimitIntervalSec = 0;
    serviceConfig = {
      Type = "simple";
      ExecStart = provision;
      Restart = "on-failure";
      RestartSec = "20s";
      # The "is this row driverless yet" state, one file per queue.  Under /run because
      # it is a fact about this boot's CUPS, which is itself re-provisioned every boot
      # (root is tmpfs) — a state file that outlived the queue it describes would have
      # `provision_one` skip a registration that does not exist.
      RuntimeDirectory = "paper-printers";
    };
  };

  config.assertions = [
    {
      assertion = badRecords == [];
      message = ''
        ./printers.nix dials a device common/dns-records.nix no longer answers: rows ${
          builtins.toJSON (builtins.filter missingRecord queues)
        } name records ${
          builtins.toJSON (map (n: PRINTERS.${n}.record) (builtins.filter missingRecord queues))
        }, and the address comes out of that record — so this cannot be repaired by
        editing an address here. See docs/paper-printers.md.'';
    }
    {
      # The URI builder's sentinel, asserted rather than trusted: it is what turns a
      # row whose record is gone into an obviously-broken queue, and this is what
      # refuses it before a user does.
      assertion =
        builtins.all
        (n: !has "no-record-in-common-dns-records.nix" PRINTERS.${n}.deviceUri)
        queues;
      message = ''
        a queue in ./printers-data.nix dialls the sentinel address, i.e. its record is
        not in common/dns-records.nix: ${
          builtins.toJSON (builtins.filter missingRecord queues)
        }'';
    }
    {
      # An endpoint that is not a path is a URI that parses and dialls the port's root,
      # i.e. a queue that answers nothing and blames the app.
      assertion =
        builtins.all (n: builtins.substring 0 1 PRINTERS.${n}.path == "/") queues;
      message = ''
        a row of ./printers-data.nix has a `path` that is not a path: ${
          builtins.toJSON (builtins.filter (n: builtins.substring 0 1 PRINTERS.${n}.path != "/") queues)
        }'';
    }
    {
      # Two defaults is one decision written twice, and nixpkgs would apply whichever
      # lpadmin runs last.
      assertion = builtins.length defaults <= 1;
      message = ''
        more than one row of ./printers-data.nix says `default = true`: ${
          builtins.toJSON defaults
        }'';
    }
    {
      # The queue name is what a desktop app shows and what `lp -d` takes, so it has to
      # be the row's key: a row that renamed its own queue makes "print to the toshiba"
      # mean two things, hours from here and with every check green.
      assertion = builtins.all (n: PRINTERS.${n}.record == n) queues;
      message = ''
        a queue in ./printers-data.nix is named differently from the record it dialls: ${
          builtins.toJSON (builtins.filter (n: PRINTERS.${n}.record != n) queues)
        }'';
    }
    {
      # `everywhere` only ever talks IPP, so a row cannot claim a scheme that is not
      # IPP: a `smb:` or `usb:` row would be a different transport with no driver, and
      # would build, boot and print nothing.  The two spellings are the ones
      # `lpadmin -v` accepts: `ipp` and `ipps`.  There is no `net+ipp`/`net+ippt` —
      # measured 2026-11-21, `net+ippt://10.3.2.30:631/ipp/print` was refused with
      # `lpadmin: Bad device-uri scheme "net+ippt"`, and because the queue registration
      # ran inside `cups.service`'s post-start with `set -e` that one row took the whole
      # scheduler down (docs/paper-printers.md).
      assertion =
        builtins.all (n: builtins.elem PRINTERS.${n}.scheme ["ipp" "ipps"]) queues;
      message = ''
        a row of ./printers-data.nix is not an IPP scheme `lpadmin -v` accepts (ipp,
        ipps), and `model = everywhere` can only drive IPP: ${
          builtins.toJSON (map (n: PRINTERS.${n}.scheme) queues)
        }'';
    }
    {
      # `everywhere` needs an IPP endpoint *and* a queue name in the path for a CUPS
      # share, so a row cannot leave the path empty and hope the scheme is enough.
      assertion = builtins.all (n: PRINTERS.${n}.path != "") queues;
      message = ''
        a row of ./printers-data.nix has no endpoint path: ${
          builtins.toJSON (builtins.filter (n: PRINTERS.${n}.path == "") queues)
        }'';
    }
  ];
}

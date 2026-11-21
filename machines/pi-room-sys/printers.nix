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
# Two things make this worth its own file rather than lines in configuration.nix, and
# both are the reason the assertions below exist:
#
# * The address of a printer is common/dns-records.nix's fact, and an image that
#   spelled its own copy of it would keep building after the record moved — printing
#   into a queue that dials a dead address, which is invisible until someone tries to
#   print. So a row names its *record* and ./printers-data.nix assembles the URI from
#   it; the assertion here is what refuses the image when the record is gone, and
#   machines/gw/lib/check-network-tables.nix carries the same rule on the eval-only
#   side, where it prints the record's name on the router.
# * This image has no disk: root is tmpfs and `/var/lib/cups` — where nixpkgs'
#   `hardware.printers` module runs the `lpadmin` that makes a queue exist — is
#   therefore re-provisioned on every boot, and nothing persists across one. That is
#   why startWhenNeeded is forced off below: left at the module's default, CUPS is
#   socket-activated, so the queues get registered *when the first connection arrives*
#   — i.e. inside the first print job rather than before it — and on an image nobody
#   ever boots twice in a row, "wait for a reboot to pick up a changed printer" is
#   wait forever. restartTriggers is the other half of the same fact.
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
in {
  config.services.printing.enable = true;

  # Never socket-activated, for the reason in the header.  The module's own `drivers`
  # list stays empty on purpose: the vendors' PPDs are not in nixpkgs, and
  # `model = everywhere` (./printers-data.nix) asks the device instead of needing a
  # driver, so `cups-filters` + ghostscript — which the module adds unconditionally —
  # is the whole filter chain this flake can build.
  config.services.printing.startWhenNeeded = false;
  # The table is a restart trigger, so an edited row re-registers the queues at the
  # next activation rather than sitting in `printers.conf` under the same name. nixpkgs
  # already restarts `cups` when `ensurePrintersLoaded` changes, and the rendered unit
  # proves both halves cross — `X-Restart-Triggers-cups` carries this file's store path
  # (receipt in docs/paper-printers.md) — so this is a superset of that trigger rather
  # than the only one: it also fires when only a `description` or `location` changed.
  config.systemd.services.cups.restartTriggers = [
    (pkgs.writeText "paper-printers-table" (builtins.toJSON PRINTERS))
  ];

  config.hardware.printers.ensurePrinters =
    map (n: {
      name = n;
      deviceUri = PRINTERS.${n}.deviceUri;
      model = "everywhere";
      description = PRINTERS.${n}.description;
      location = PRINTERS.${n}.location;
      ppdOptions = {};
    })
    queues;

  # A default is a decision, not a fact, so the table owns it and `null` — no default,
  # the app asks — is the legal answer this image gives today: nothing measured says
  # which of the two printers a Pi Room desk should get when nobody names one, and
  # picking one on a hunch is a document leaving for the wrong machine.
  config.hardware.printers.ensureDefaultPrinter =
    if defaults == []
    then null
    else builtins.head defaults;

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
      # would build, boot and print nothing.  The four spellings are CUPS's own —
      # `ipp` for a queue inside a CUPS-shaped server, `ippt` for an endpoint on the
      # device, each with a `net+` form that skips the device-side checks.
      assertion =
        builtins.all
        (n:
          builtins.elem PRINTERS.${n}.scheme [
            "ipp"
            "ippt"
            "net+ipp"
            "net+ippt"
          ])
        queues;
      message = ''
        a row of ./printers-data.nix is not IPP, and `model = everywhere` can only
        drive IPP: ${builtins.toJSON (map (n: PRINTERS.${n}.scheme) queues)}'';
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

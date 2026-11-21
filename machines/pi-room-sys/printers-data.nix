# DATA — the network's two PAPER printers, as this image dials them.
#
# Not the 3D print servers: `docs/printer-servers.md` owns those (3d-blue/3d-lime,
# Moonraker on :7125), and nothing here has to do with them.  This file is the
# sheet-of-paper end of the network — the two devices the Pi Room's PCs are meant
# to print to — and `docs/paper-printers.md` owns the receipts, the proven/guessed
# split and the open items.  Read that before changing a row.
#
# **There is no address in here, and no second spelling of one.**  Which device keeps
# which address is `../../common/dns-records.nix`'s fact — the same reason a printer
# monitor could not write its own IPv4s without a guard: the router answers the name
# and the queue dials the address, and if the two spellings drift the queue prints
# into a dead address hours from wherever the move was made.  A row therefore names
# its *record* and `deviceUri` is assembled here from that record's IPv4, so the
# image, the assertion in `./printers.nix` and `just paper-printers` all read the one
# expression rather than three copies of a URI.  `machines/gw/lib/check-network-tables.nix`
# refuses a row whose record is gone, from `just check-eval` as well as from a switch
# of this image, and prints the record's name rather than a store path.
#
# The keys are the CUPS queue names the desktop apps show, and they are the record
# names, so a printer has one name from the router's lease table through to `lp -d`.
# `./printers.nix` asserts the two cannot be made to disagree.
#
# Every value below is marked with the request that put it there (measured 2026-11-21,
# full receipts in docs/paper-printers.md) or with the word GUESS.  The scheme is not
# a style choice either: `lpadmin -v` accepts `ipp`/`ipps` and nothing else, and
# `net+ippt` (a spelling this table used to carry) is refused outright.
let
  RECORDS = import ../../common/dns-records.nix;

  ROWS = {
    brother = {
      record = "brother";

      # PROVEN IPP-native: `GET :631/ipp/print` answers 405 — the path exists, and
      # POST is the only method it takes — and a 10-byte IPP Get-Printer-Attributes
      # POST to the same path returns 200.  `GET :631/printers/` and `:nic/nicFilter`
      # are 404, so it is not a share of a CUPS server and it is not DNDM.  The scheme
      # is plain `ipp`, because that is the spelling `lpadmin -v` takes for *any* IPP
      # endpoint; the `net+ippt` this row used to carry is not a device-uri scheme at
      # all — `lpadmin` refused it, and that is the fault that stopped every printer
      # appearing (measured on a booted client 2026-11-21, docs/paper-printers.md).
      scheme = "ipp";
      port = 631;
      path = "/ipp/print";

      # VLAN 226, by its record.  Nothing physical is claimed here — which desk it
      # lives by is not something this image can see, and a wrong room is a queue a
      # user picks at the wrong machine, silently.
      description = "The network's `brother` printer (WiFi VLAN)";
      location = null;
      default = false;
    };

    toshiba = {
      record = "toshiba";

      # PROVEN as to its shape: `OPTIONS :631/printers/` returns 200 with
      # `Server: CUPS/1.5` and `Allow: GET, HEAD, OPTIONS, POST, PUT`, while
      # `/ipp/print` is 404 on both :80 and :631 and `/nic/*` is 404 — and its :80
      # serves TopAccess (`gblTAUrl`, `eFiling`), i.e. a Toshiba e-STUDIO.  So the
      # endpoint is a CUPS-shaped share, hence the `ipp` scheme with a queue path.
      scheme = "ipp";
      port = 631;

      # GUESS — the one value in this table that is not measured, and the queue name
      # the row cannot be caught lying about by anything on the network: that server
      # sends 0 bytes for `GET /printers/`, `/admin/` and `/jobs/`, refuses a POST the
      # same way, and answers `OPTIONS /printers/<anything>` with 200 for *every* name,
      # so there is no enumeration to read.  Settling it is one line here, and the
      # truth is in some queue that already works — see the open item in
      # docs/follow-ups.md.  Until then this queue exists, enables, and prints
      # nothing, which is the failure mode every check inside CUPS stays green through.
      #
      # It also has no queue at all while the device is off, and that is not a bug in
      # this row: `-m everywhere` asks the device for its attributes when the queue is
      # registered, so the row is retried until it answers (`Host is down` on
      # 2026-11-21, docs/paper-printers.md).
      path = "/printers/toshiba";

      description = "The network's `toshiba` printer (untagged lease)";
      location = null;
      default = false;
    };
  };

  # The device URI the queue dials: the row's scheme and endpoint, the address read
  # out of the record it names.  A missing record puts the queue at an address that
  # cannot exist rather than throwing here, because a throw would die in the evaluator
  # with this file's path; the assertions in ./printers.nix and
  # machines/gw/lib/check-network-tables.nix are what name the record.
  mkUri = n: r: "${r.scheme}://${(
    RECORDS.${
      r.record
    } or {
      ipv4 = "no-record-in-common-dns-records.nix";
    }
  ).ipv4}:${builtins.toString r.port}${r.path}";
in
  builtins.mapAttrs (n: r:
    r
    // {
      deviceUri = mkUri n r;
    })
  ROWS

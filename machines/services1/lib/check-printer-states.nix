# check-printer-states.nix
#
# Assertions over ./printer-states.nix — the klippy / print_stats state tables
# that are read out of moonraker-exporter/src/main.rs and used to generate the
# Grafana value mappings (services/monitoring-dashboards.nix) and the Prometheus
# alert numbers (services/monitoring.nix).  Run it with:
#
#   nix eval --impure --raw -f machines/services1/lib/check-printer-states.nix
#
# It prints "ok" or throws.  `just check` runs it.  Three things are covered:
#
#   1. the two Rust arrays still parse, and still parse into the numbering the
#      deployed dashboards were built with — that pin is spelled out below, so
#      reordering an array fails the check instead of silently renumbering every
#      panel and alert rule;
#   2. the generated mapping/code attrsets are inverses of each other and never
#      claim the exporter's "unknown state" code;
#   3. the dashboards and the alert rules still *use* the generated tables rather
#      than a hand-written copy (the drift this replaced, §3.6 of the repo audit).
#
# Substring tests use replaceStrings rather than builtins.match: match is a POSIX
# ERE over the whole file and the strings being looked for are full of regex
# metacharacters.
let
  PRINTER = import ./printer-states.nix;

  monitoring = builtins.readFile ../services/monitoring.nix;
  dashboards = builtins.readFile ../services/monitoring-dashboards.nix;

  has = sub: src: builtins.replaceStrings [sub] [""] src != src;

  need = cond: msg:
    if cond
    then true
    else builtins.throw "check-printer-states: ${msg}";

  # State labels go into the `state=` tag of the metric and into PromQL string
  # literals, so they have to stay bare lowercase words.
  labelOk = l: builtins.match "[a-z_]+" l != null;

  checkTable = kind: t: let
    n = builtins.length t.labels;
    unique =
      builtins.length (builtins.attrNames (builtins.listToAttrs (builtins.map
        (l: {
          name = l;
          value = true;
        })
        t.labels)))
      == n;
    roundTrip =
      builtins.all
      (i:
        t.mapping.${builtins.toString i}
        == builtins.elemAt t.labels i
        && t.code.${builtins.elemAt t.labels i} == i)
      (builtins.genList (i: i) n);
  in
    need (n > 0) "${kind}: no states parsed out of moonraker-exporter/src/main.rs"
    && need (builtins.all labelOk t.labels)
    "${kind}: a state label is not [a-z_]+: ${builtins.toJSON t.labels}"
    && need unique "${kind}: duplicate state label in ${builtins.toJSON t.labels}"
    && need roundTrip "${kind}: mapping and code are not inverses"
    && need (builtins.all (l: t.code.${l} != PRINTER.unknownCode) t.labels)
    "${kind}: a known state collides with the exporter's unknown-state code 99";

  # The numbering as deployed today (the exporter's own tests pin the same
  # numbers from the other side).
  expect = kind: table: wanted:
    need
    (builtins.all
      (name: (table.code.${name} or null) == builtins.getAttr name wanted)
      (builtins.attrNames wanted)
      && builtins.length table.labels == builtins.length (builtins.attrNames wanted))
    "${kind} numbering changed: ${builtins.toJSON table.code}";
in
  builtins.seq
  (
    checkTable "klippy" PRINTER.klippy
    && checkTable "print" PRINTER.print
    && expect "klippy" PRINTER.klippy {
      startup = 0;
      ready = 1;
      error = 2;
      shutdown = 3;
      disconnected = 4;
    }
    && expect "print" PRINTER.print {
      standby = 0;
      printing = 1;
      paused = 2;
      complete = 3;
      cancelled = 4;
      error = 5;
    }
    # The consumers must go through the generated tables...
    && need
    (has "PRINTER.klippy.code.error" monitoring
      && has "PRINTER.klippy.code.shutdown" monitoring)
    "monitoring.nix no longer derives the klippy alert numbers from printer-states.nix"
    && need
    (has "PRINTER.klippy.mapping" dashboards
      && has "PRINTER.print.mapping" dashboards)
    "monitoring-dashboards.nix no longer uses the generated state mappings"
    # ... and must not have grown a hand-written copy back.
    && need
    (!has "\"0\" = \"startup\"" dashboards && !has "\"0\" = \"standby\"" dashboards)
    "monitoring-dashboards.nix contains a hand-written state table again"
    # A hardcoded alert number would be the other way to drift back.
    && need
    (!has "state=\\\"error\\\"} == 2" monitoring
      && !has "state=\\\"shutdown\\\"} == 3" monitoring)
    "monitoring.nix hardcodes a klippy state number again"
  )
  "ok\n"

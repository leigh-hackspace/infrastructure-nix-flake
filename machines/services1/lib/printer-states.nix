# printer-states.nix — the printer state enums, read out of the exporter itself.
#
# moonraker-exporter emits `moonraker_klippy_state` / `moonraker_print_state` with
# the state as a `state=` tag and the *index into its own table* as the value.
# The Grafana value mappings (services/monitoring-dashboards.nix) and the
# Prometheus alert rules (services/monitoring.nix) used to restate those tables by
# hand — three copies kept in step by a comment, which is exactly the drift
# docs/repo-audit-2026-10-07.md §3.6 calls out.  They are generated from the
# exporter instead:
#
#   PRINTER = import ../lib/printer-states.nix;
#
#   PRINTER.klippy.labels    # [ "startup" "ready" ... ]
#   PRINTER.klippy.mapping   # { "0" = "startup"; "1" = "ready"; ... }  Grafana
#   PRINTER.klippy.code      # { startup = 0; ready = 1; ... }          PromQL
#   PRINTER.print.*          # same three, for print_stats.state
#
# The source of truth is the two `const ... &[&str]` arrays in
# moonraker-exporter/src/main.rs.  If they change *contents*, the panels and the
# alert numbers follow on the next switch.  If they change *shape*, evaluation
# throws here rather than quietly rendering a dashboard with no mappings.
#
# `just check` runs ./check-printer-states.nix, which pins the current numbering
# so an accidental reorder in the Rust arrays is a check failure, not a dashboard
# that silently relabels every printer.
let
  source = builtins.readFile ../../../moonraker-exporter/src/main.rs;

  # builtins.match is POSIX ERE: anchored over the whole file, no `\[` escape (the
  # bracket expressions `[[]` and `[].]` match the literal brackets), and `.`
  # spans newlines so a rustfmt-wrapped array still parses.  `[^;]*` keeps the
  # match inside the one `const NAME ... ;` statement.
  parseConst = name: let
    m = builtins.match ".*const ${name}[^;]*[[]([^]]*)[].].*" source;
  in
    if m == null
    then
      builtins.throw ''
        printer-states.nix: could not find `const ${name}: &[&str] = &[...]` in
        moonraker-exporter/src/main.rs.  The Grafana mappings and the Prometheus
        alert numbers are generated from that array, so it has to keep that name
        and shape (see lib/printer-states.nix).
      ''
    else builtins.head m;

  # `"a", "b"` -> [ "a" "b" ]: split on commas, then drop the quotes and the
  # whitespace rustfmt puts in.  Moonraker's state labels are single words, so
  # stripping every space is safe (asserted in check-printer-states.nix).  The
  # empty-string filter is for the trailing comma rustfmt adds when it wraps a
  # long array onto several lines.
  labels = name:
    builtins.filter (s: s != "")
    (builtins.map (s: builtins.replaceStrings ["\"" " " "\t" "\n"] ["" "" "" ""] s)
      (builtins.filter builtins.isString (builtins.split "," (parseConst name))));

  table = name: let
    ls = labels name;
    n = builtins.length ls;
  in {
    labels = ls;
    mapping = builtins.listToAttrs (
      builtins.genList
      (i: {
        name = builtins.toString i;
        value = builtins.elemAt ls i;
      })
      n
    );
    code = builtins.listToAttrs (
      builtins.genList
      (i: {
        name = builtins.elemAt ls i;
        value = i;
      })
      n
    );
  };

  klippy = table "KLIPPY_STATES";
  print = table "PRINT_STATES";
in {
  inherit klippy print;

  # The exporter codes anything outside its table as 99; the alert rules and
  # panels must never claim that value for a known state.
  unknownCode = 99;
}

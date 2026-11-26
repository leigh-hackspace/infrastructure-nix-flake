# CHECK — plain-eval assertion file: `nix eval --impure --raw -f` prints ok or throws.
#
# Assertions over `common/radio-dongles.nix`, through `common/radio-tables.nix`:
# every 802.15.4 radio this network owns, its role, and the node it is plugged
# into. Run it with:
#
#   nix eval --impure --raw -f common/check-radio-tables.nix
#
# It prints "ok" or throws with the offending values. The predicates themselves
# live in `common/radio-tables.nix`, so `just check-eval` and
# `machines/services1/services/thread-router.nix` assert one list rather than
# two copies of the rules. The two things that make this an eval and not a
# binary are the ones `common/check-doc-pointers.nix`'s header states: it has to
# run as a step of `just check` on every box including the router, ~0 s and no
# build, and a binary could not promise that.
#
# WHAT A PASS DOES NOT PROVE: that the dongle named in the table is the dongle
# in the USB port. That is a physical fact, and `just radio-status` answers it
# against the box. A green eval plus a dongle that has been swapped is exactly
# the state the guard cannot see, and it is why the verb exists.
let
  TABLES = import ./radio-tables.nix;

  need = cond: msg:
    if cond
    then true
    else builtins.throw "check-radio-tables: ${msg}";
in
  assert (
    need (TABLES.problems == []) (builtins.concatStringsSep "; " TABLES.problems)
  ); "ok\n"

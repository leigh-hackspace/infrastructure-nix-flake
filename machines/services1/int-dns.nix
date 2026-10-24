# The **origin guard**: the vhosts this box still serves must be exactly the
# `upstream = "origin"` rows in `common/front-door.nix`.
#
# This file used to assert that this box's nginx served every `*.int` name in
# `common/int-dns.nix`, because services1 was the front door. It is not any more:
# `gw` terminates TLS, holds the certificate, runs the gate and `anubis`, and answers
# the `*.int` zone (machines/gw/int-dns.nix is the guard for that half). What is left
# here is the *origin* — the handful of vhosts whose applications are app-shaped (a
# `/srv` root, an auth rewrite, a filesystem mount) rather than one-line proxies.
#
# So the invariant changes shape with the box, and this is the shape that keeps it
# honest:
#
#   * every vhost on this box (including `serverAliases`) is an origin row in the
#     front-door table, and
#   * every origin row in the table has a vhost here.
#
# Equality, not subset: a direct row is served by the door itself, so a stale copy
# here would be a second config for the same name — the drift this migration exists
# to remove. A name moved from `origin` to a direct address, or an origin row added
# without its vhost, fails the evaluation. That is AGENTS.md rule 24 in its new
# position: a vhost move is a guard move, and the guard moved with the vhosts.
#
# It runs wherever this machine is evaluated — every `just switch`, `nixos-rebuild
# dry-run` and `just check-build` — which is the same coverage the old guard had.
{
  config,
  lib,
  ...
}: let
  TABLE = import ../../common/front-door.nix {
    hosts = import ../../common/hosts.nix;
  };

  # Names the door sends to this box's origin nginx, plus the aliases that share the
  # same server block. A row's `serverAliases` are names the origin must also answer,
  # because the door forwards the client's `Host` untouched.
  originRows = builtins.filter (n: (TABLE.names.${n}.upstream or null) == "origin") (builtins.attrNames TABLE.names);
  declared =
    lib.sort (a: b: a < b)
    (builtins.concatLists (map (n: [n] ++ (TABLE.names.${n}.serverAliases or [])) originRows));

  # Every name this box's nginx serves, from its vhost list (the same derivation the
  # front-door guard uses, read the other way round).
  served =
    lib.sort (a: b: a < b)
    (builtins.concatLists (
      map
      (n: [n] ++ (config.services.nginx.virtualHosts.${n}.serverAliases or []))
      (builtins.attrNames config.services.nginx.virtualHosts)
    ));

  missing = builtins.filter (n: !builtins.elem n declared) served;
  extra = builtins.filter (n: !builtins.elem n served) declared;
in {
  assertions = [
    {
      assertion = served == declared;
      message = ''
        machines/services1/http.nix (and the service modules' vhosts) no longer match
        the `upstream = "origin"` rows in common/front-door.nix.

          served here, not an origin row: ${builtins.toJSON missing}
          an origin row, not served here: ${builtins.toJSON extra}

        An origin is a vhost on this box that `gw` proxies to over plain HTTP; a
        direct row is one `gw` proxies straight to the box that runs the app. Moving a
        name between the two is one change in `common/front-door.nix` plus the vhost
        here, and the certificate/gate/ACL never come back to this box.  See
        docs/router-front-door-migration.md phase 3.
      '';
    }
  ];

  # The same list rendered, for the moment it is not agreeing.
  environment.etc."int-dns/served-origin-names".text = lib.concatStringsSep "\n" served + "\n";
}

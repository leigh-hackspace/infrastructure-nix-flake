# The router's half of the front door's guard: the *assertion* over
# `common/int-dns.nix` read from this box's nginx, the mirror of
# `machines/services1/int-dns.nix` that AGENTS.md rule 24 demands ("a vhost move is a
# guard move").
#
# Why this is a module and not a third entry in `just check`.  The file it belongs
# next to is ./lib/check-network-tables.nix, which is a standalone evaluator
# precisely so it can be run without deploying anything — but what is asserted here
# is a comparison against `services.nginx.virtualHosts`, and those only exist inside
# a machine's config.  So: ./lib/check-network-tables.nix checks the *tables* (and
# is the one `just check` can run on the router), and this file checks the *config*
# and runs wherever this machine is evaluated — every `just deploy gw`,
# `nixos-rebuild dry-run` and `just check-build`, plus `nix flake check`, which
# evaluates all five configurations.  Neither replaces the other, and the argument
# for the pair is the same one that file makes: this is the check that **cannot be
# fixed by deploying**, because the file that would be wrong —
# `/etc/dnsmasq-int-vhosts.hosts` — is generated from the same table this compares
# against.
#
# WHY THE RULE CHANGES SHAPE, which is the whole reason this file is not a copy of
# services1's guard. `common/int-dns.nix`'s `frontDoor` names this box, so
# "the table equals the names this box serves" is the honest invariant **here**,
# and it is equality in both directions: a name in the table that no vhost here
# serves is an NXDOMAIN-shaped 404 at the front door, and a vhost here whose name
# is not in the table is a name that resolves somewhere else while this box answers
# it. During the move the guard also permitted a subset, so the router could serve
# a name before the answers pointed at it; that mode is gone, because there is no
# second front door to pre-stage against any more.
#
# The origin's half is `machines/services1/int-dns.nix`, which compares that box's
# vhosts to the `upstream = "origin"` rows of `common/front-door.nix` instead. The
# pair covers both kinds of box, and each fails an evaluation rather than a client.
{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../common/config.nix;
  INT = import ../../common/int-dns.nix;
  intVhostNames = import ../../common/nginx/int-vhost-names.nix {inherit lib;};

  zone = CONFIG.LOCAL_DOMAIN;

  # Every `*.int` name this box's nginx is configured to serve, FQDNs, sorted —
  # vhost names plus their serverAliases, the same derivation services1's guard and
  # the gatus endpoint guard use, so "this box serves it" means one thing in three
  # places.
  served = intVhostNames config.services.nginx.virtualHosts;

  # What the network is told the front door serves: the front-door list, plus the
  # names answered elsewhere on purpose (so the comparison can tell "moved out of the
  # zone" from "forgotten" — `machines/gw/lib/check-network-tables.nix` refuses an
  # `exempt` name that is not a device record, so it can never hold a typo).
  declared =
    lib.sort (a: b: a < b)
    (
      (map (n: "${n}.${zone}") INT.names)
      ++ (map (n: "${n}.${zone}") (builtins.attrNames INT.exempt))
    );

  # Is the network's answer this box?  `common/int-dns.nix`'s `frontDoor` is the one
  # fact that decides it, and ./lib/network-tables.nix's `doorAddresses` is the
  # rendering that acts on it — this comparison and that one read the same string so
  # they cannot disagree about which mode the guard is in.
  isDoor = INT.frontDoor == "gw";

  # Names served here that the network does not answer at any front door.
  invented = builtins.filter (n: !builtins.elem n declared) served;
  # Names the network answers at the front door that nothing here serves.
  unanswered = builtins.filter (n: !builtins.elem n served) declared;
in {
  assertions = [
    {
      # Runs in both modes: a name nginx serves here and the table does not answer is
      # wrong whether or not this box is the door.
      assertion = invented == [];
      message = ''
        This box's nginx serves `*.int` names that common/int-dns.nix does not answer
        at the front door: ${builtins.toJSON invented}

        The router renders that table into /etc/dnsmasq-int-vhosts.hosts, so a name
        served here and absent from the table resolves wherever it resolved before —
        which, for a name that is only in this file, is nowhere.  Add the label to
        `names` (and then, if this box is the door, every other name the table answers)
        or delete the vhost.  A `*.int` name served here that must *not* be answered at
        the front door belongs in `exempt`, with the reason and the device record that
        answers it.
      '';
    }
    {
      # Only when the answers point here: the door must serve everything it is dialled
      # for.  While services1 is the door this is deliberately permitted — that is the
      # migration's middle state (docs/router-front-door-migration.md), not a bug.
      assertion = !isDoor || unanswered == [];
      message = ''
        common/int-dns.nix says this box *is* the front door (frontDoor = "gw"), but
        these names it answers at 10.3.1.1 are served by no vhost here:
        ${builtins.toJSON unanswered}

        Every one of them is a client that resolves the name, connects to this box and
        gets nothing, while the vhost that does serve it sits on a box no name resolves
        at.  Move the rest of them into machines/gw/http.nix, or move `frontDoor` back
        to the box that serves them.  The order that makes this safe — cert, vhosts,
        guards, then the answer — is in docs/router-front-door-migration.md.
      '';
    }
  ];

  # The same list the assertion compares, rendered for the moment it is not agreeing:
  # diff it against the router's own /etc/dnsmasq-int-vhosts.hosts and the difference
  # *is* the finding, with the address in it.  Same path services1 writes (see
  # docs/router-and-dns.md, "Reading what the router answers"); which box to read it
  # on is the thing that changes when the door moves.
  environment.etc."int-dns/served-int-names".text = lib.concatStringsSep "\n" served + "\n";
}

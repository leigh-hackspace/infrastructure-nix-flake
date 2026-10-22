# The "*.int.leighhack.org" names a box's nginx serves: every vhost name plus
# every serverAlias ending in that suffix.  Two readers need it to mean the same
# thing: the gatus endpoint guard compares its internal-app checks against it, and
# `machines/services1/int-dns.nix` refuses to evaluate unless it equals the
# front-door list in common/int-dns.nix — the table the router answers this zone
# from — so "this box serves it" and "the network resolves it" cannot drift apart.
#
#   intVhostNames = import common/nginx/int-vhost-names.nix { inherit lib; };
#   intVhostNames config.services.nginx.virtualHosts   # -> sorted, unique names
{lib}: virtualHosts:
lib.sort (a: b: a < b) (
  lib.unique (
    lib.filter (n: lib.hasSuffix ".int.leighhack.org" n) (
      lib.flatten (
        lib.mapAttrsToList (
          name: vh: [name] ++ lib.toList (vh.serverAliases or [])
        )
        virtualHosts
      )
    )
  )
)

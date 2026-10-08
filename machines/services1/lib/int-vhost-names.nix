# The "*.int.leighhack.org" names this box's nginx serves: every vhost name plus
# every serverAlias ending in that suffix.  This is the set dns-sync makes sure
# the router and DigitalOcean have records for, and the set the gatus endpoint
# check compares its internal-app checks against — one derivation, so the two
# cannot disagree about what "an internal app on this box" means.
#
#   intVhostNames = import ./lib/int-vhost-names.nix { inherit lib; };
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

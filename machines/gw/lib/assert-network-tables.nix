# assert-network-tables.nix
#
# The module entry point to ./check-network-tables.nix's assertions.  It is a
# three-line wrapper rather than a copy of them: the assertions are one set, and
# they run from two places — `just check` (which evaluates the standalone file
# directly, so a finding is a message rather than an eval error) and every
# evaluation of this machine's config, which is what stops a router being built
# from tables that disagree.
#
# Read ./check-network-tables.nix first: it owns the assertions, and it says why
# they exist.
{
  config,
  lib,
  ...
}: {
  assertions = [
    {
      assertion = (import ./check-network-tables.nix) == "ok\n";
      message = ''
        machines/gw/lib/check-network-tables.nix: the network's tables
        (common/vlans.nix, common/dns-records.nix) and the router's reading of
        them disagree.  Run `nix eval --impure --raw -f
        machines/gw/lib/check-network-tables.nix` for the finding — the module
        assertion cannot carry it, and the finding names the values.
      '';
    }
  ];
}

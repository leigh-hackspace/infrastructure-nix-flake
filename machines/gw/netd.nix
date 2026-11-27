# MODULE — gw's netd: the replacement for dnsmasq, rendered from the same tables.
#
# **`enable = false`**, deliberately, and that is the cutover switch rather than a
# half-finished file: the daemon runs, is tested and is built from this flake
# (`just netd-check`), but `services.dnsmasq` still owns ports 53 and 67 on this
# box. What is live *now* is the rendering — `./netd-config.nix` reads every value
# from a table, so a VLAN, a reservation or a front-door name that changes shows up
# in the document in the same deploy, and `./lib/check-netd-config.nix` refuses a
# render that dropped one. That check is what `just check-eval` runs on this box
# (`docs/nix-dev-workflow.md`: the router's own gate is the eval half).
#
# `docs/netd-2026-11-26.md` is the plan, the reasons and the runbook. The short
# form: the daemon serves exactly the twenty-three directives this network used;
# the lease table becomes an API instead of a file (`./lease-reader.nix` exists
# because a file another daemon writes is a channel that can die); and the
# router-advertisement timer is a comparison in a loop rather than a timer inside a
# daemon (`./ra-watch.nix` exists because dnsmasq's died on three link flaps).
#
# ## The cutover, when it happens
#
# One commit, three lines:
#
#     machines/gw/netd.nix:     services.netd.enable = true;
#     machines/gw/dns.nix:      services.dnsmasq.enable = false;
#     machines/gw/default.nix:  drop ./ra-watch.nix and ./lease-reader.nix
#
# ...and then read the runbook in the doc rather than watching the switch. The
# assertions in `common/netd/module.nix` refuse to build with both daemons enabled,
# which is the one mistake that would otherwise be silent.
{...}: {
  imports = [../../common/netd/module.nix];

  services.netd = {
    enable = false;
    config = import ./netd-config.nix;
  };
}

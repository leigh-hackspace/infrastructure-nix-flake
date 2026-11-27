# HELPER — gw's netd configuration, as a value. Not a module; `import`ed.
#
# Split from `./netd.nix` so the document can be read by two callers that cannot
# see each other's internals: the machine's module (which hands it to
# `services.netd.config`) and `./lib/check-netd-config.nix` (which
# `just check-eval` and `nix flake check` evaluate). One call site, so the
# arguments are written once — the thing a second copy would get wrong is exactly
# what the check is for.
#
# Every argument below is an existing table, and the two `upstreams`/`conditional`
# lists are the only facts written here rather than read: they are dnsmasq's own
# four resolvers (`machines/gw/dns.nix`) and the tailnet's conditional forward,
# and when the cutover happens this file is the only place they will be written.
let
  CONFIG = import ../../common/config.nix;
  HOSTS = import ../../common/hosts.nix;
  VLANS = import ../../common/vlans.nix;
  RECORDS = import ../../common/dns-records.nix;
  INT = import ../../common/int-dns.nix;
in
  (import ../../common/netd/mk-config.nix) {
    tables = import ./lib/network-tables.nix;
    vlans = VLANS;
    records = RECORDS;
    hosts = HOSTS;
    int = INT;
    zone = CONFIG.LOCAL_DOMAIN;

    # The four upstreams `machines/gw/dns.nix` lists, in the order dnsmasq's
    # `strict-order` asks them; and the one suffix that belongs to the tailnet's
    # own resolver rather than to a public one.
    upstreams = [
      "9.9.9.9"
      "8.8.8.8"
      "1.1.1.1"
      "8.8.4.4"
    ];
    conditional = [
      {
        zone = "ts.leighhack.org";
        servers = ["100.100.100.100"];
      }
    ];

    apiBind = HOSTS.router;
    apiPort = 8092;

    # The intercom's address: a *second* address on services1's vlan225, held by
    # the NIC already reserved as `.20`, inside VLAN 225's dynamic range.
    # `machines/gw/dns.nix` carries the story; the two are one fact and must move
    # together.
    ignoredAddresses = ["10.3.1.166"];

    # The APs are all reserved on this VLAN and none is a network of this flake,
    # so a label from `common/hosts.nix` rather than an id.
    unifiController = "services1";
    tftpServer = HOSTS.aibox;
  }

# DATA — plain attrset read by `import` from anywhere in the tree; it is not a module and defines no options.
# Every `*.int.leighhack.org` name the network answers, and with what.
#
# This is the DNS half of the front door.  `machines/gw/dns.nix` renders it into
# `/etc/dnsmasq-int-vhosts.hosts` — one of dnsmasq's two `addn-hosts` files — and
# `machines/gw/int-dns.nix` asserts that `names` plus `exempt` is *exactly* the set
# of `*.int` names the front door's nginx serves (vhost names plus their
# serverAliases, derived from `services.nginx.virtualHosts` by
# `common/nginx/int-vhost-names.nix`).  `machines/services1/int-dns.nix` is the
# other half: it asserts that the origin box's vhosts are exactly the
# `upstream = "origin"` rows in `common/front-door.nix`.  So the table and the boxes
# that serve it can only disagree by failing an evaluation.  Until 2026-10-22 the
# same set was pushed onto the router by a tool (`crates/dns-sync`) that edited a
# mutable file over ssh when someone remembered to run it.  Nothing was broken the
# day it was deleted — every name the deployed nginx served had its record — and
# that is precisely the debt being paid off: correctness that depended on a habit, a
# file no deploy could reproduce, and a checker that was itself a manual command.
#
# The rule for a new internal app: add the vhost on services1, add its `*.int`
# name here, deploy.  Nothing else, no command, no router-side state.  Both
# halves are checked at deploy time — the router's because the file it serves is
# a store path and a `restartTriggers` value (`machines/gw/dns.nix`), services1's
# because its module refuses to evaluate when the two lists drift.
#
# What is deliberately *not* in here:
#
#   * **the zone suffix.** `names` are labels; the `.int.leighhack.org` comes
#     from `CONFIG.LOCAL_DOMAIN` at render time, in the one place `machines/gw/
#     dns.nix` says the zone is decided.  A table that spelled the suffix out
#     would be two facts about one name.
#   * **the addresses.** The front door's come from `frontDoor`'s entry in
#     `common/dns-records.nix` — three of them (IPv4, public IPv6, ULA) — and a
#     `fromHosts` name's come from `common/hosts.nix`.  A name missing its IPv6
#     half is the fault `docs/router-and-dns.md` documents (a dual-stack client
#     tries IPv6, hangs, and only then falls back), and it cannot happen while
#     the addresses are read rather than copied.
#   * **the devices.** A camera, an access point or a Pi Room board is not a vhost
#     and keeps its own address; those names come from `common/dns-records.nix`,
#     which is the reservation table that also answers the bare name.  The three
#     ProCurve switches are the devices that are *not* DHCP clients — they have no
#     reservation to hang a name on, which is what `fromHosts` is for.
#
# Fields
#   names      labels served by the front door (`frontDoor`'s addresses)
#   frontDoor  which box those addresses belong to — and therefore which box's nginx
#              has to serve them.  It is `gw` now (the router, whose three addresses
#              are the VLAN in `common/vlans.nix` whose `ipv4` is `hosts.router` —
#              it has no reservation and cannot have one — read by
#              `machines/gw/lib/network-tables.nix`'s `doorAddresses`).  It may also
#              name an entry of `common/dns-records.nix` for a staged move.  One
#              string, so the DNS answer and the guards cannot disagree about which
#              box the door is: `machines/gw/int-dns.nix` reads it and requires this
#              box to serve every answered name, and `machines/services1/int-dns.nix`
#              independently checks what the origin serves.
#              `docs/router-front-door-migration.md` is the architecture and the
#              deploy record.
#   fromHosts  labels that answer with their own address, taken from
#              `common/hosts.nix`: devices whose name has to resolve and that are
#              not DHCP clients, so they have no reservation to hang a record on
#   legacy     labels that answer with the front door but are served by no enabled
#              vhost.  Each one is a name this table keeps rather than drops so
#              that the deploy which made DNS declarative changed no answer; they
#              are exempt from the drift check for exactly as long as it takes to
#              delete them, and every entry says who to check first
#   exempt     `*.int` names services1's nginx serves that must *not* answer with
#              the front door, each with the reason.  They are in the list only so
#              the drift check can tell "answered elsewhere on purpose" from
#              "forgotten" — a name here that resolves at the front door is a bug
#              in whatever answers it, not in this table
{
  # The box every `*.int` name resolves to.  It moved from `services1` to `gw` on
  # 2026-10-24 with the front door (`docs/router-front-door-migration.md`): the router
  # terminates TLS, holds the certificate, runs the gate and `anubis`, and serves every
  # name in `common/front-door.nix`, so the addresses and the box serving them are one
  # fact again.  `machines/gw/int-dns.nix` reads this string to switch from "may stage a
  # subset" to "must serve every name", so the answer and the guard move together.
  frontDoor = "gw";

  names = [
    "access-api"
    "ai"
    "aibox-status"
    "alexandria"
    "api"
    "api-doors"
    "bar"
    "doors"
    "filestore"
    "firewall"
    "frigate"
    "frigate-monitor"
    "gatus"
    "gocardless"
    "grafana"
    "ha"
    "id"
    "jenkins"
    "login"
    "mcp"
    "monster"
    "network-info"
    "prometheus"
    "retro"
    "services1-status"
    "services2-status"
    "strata"
    "strata2"
    "switch"
    "truenas"
    "unifi-admin"
    "user-tweaker"
    "whisper"
    "zigbee2mqtt"
  ];

  fromHosts = [
    # The ProCurve switches.  `common/hosts.nix` calls them switch1/2/3 and
    # switch-gui and the gatus checks use those addresses; the names resolve
    # because someone has to be able to type `switch2.int` into a browser.  They
    # are not in `common/dns-records.nix` because they are not DHCP clients —
    # their addresses are configured on the switches themselves — so a
    # reservation would have no identifier to match on and
    # `machines/gw/lib/check-network-tables.nix` rightly refuses one.
    "switch1"
    "switch2"
    "switch3"
  ];

  legacy = {
    # The three names that predate the drift check and have always answered at the
    # front door without a vhost declaring them.  They are `legacy` rather than
    # `names` because nothing serves them, and the guard in
    # machines/services1/int-dns.nix is what says so — it failed its first build
    # over exactly these three, which is the whole argument for checking it at eval
    # time rather than in a comment.
    #
    # `nginx.int` is load-bearing in a way the others are not: it is the target of
    # every `*.int` CNAME in the DigitalOcean zone, so removing it would orphan
    # forty public records while changing nothing on the LAN.  It has answered at
    # 10.3.1.20 since before this repo had a word for that.
    nginx = "the CNAME target of every public *.int record in DigitalOcean; no vhost, and it must keep resolving";
    # mosquitto (machines/services1/services/mqtt.nix) publishes 1883/8883 from the
    # container and is not HTTP, so it is not fronted by nginx: the name resolves at
    # the front door and 404s. Kept because an MQTT client config somewhere may
    # dial the name, and a name that stops resolving fails more confusingly than
    # one that answers wrong.
    mqtt = "mosquitto speaks MQTT on its own ports, not HTTP; this name has never been fronted";
    # The headscale admin UI lives at `tailscale.leighhack.org` (a front-door row,
    # served by machines/gw/headscale.nix); `tailscale.int` is the name from before
    # that, with no vhost behind it.
    tailscale = "the headscale UI is `tailscale.leighhack.org` now; the int name predates that";

    # The other two names that predate any table — both answer with services1's nginx
    # and nginx 404s them — but both have resolved to the front door for longer than
    # this repo has had a name for the fact, and a name that silently stops
    # resolving is only noticed by the thing that uses it, hours later, in a log.
    # So they came across with their reason written down instead of being dropped
    # in the same breath as the sync tool.
    #
    # `gitlab.int` is the redirect vhost in
    # `machines/services1/services/gitlab.nix`, which is not enabled in
    # `services/default.nix` (the container is not deployed). When gitlab comes
    # back on, delete this entry and the vhost re-enters `names` through the drift
    # check by itself.
    gitlab = "the disabled gitlab container's redirect vhost (services/services/gitlab.nix)";
    # The authentik LDAP outpost, historically reached this way; the tree's only
    # live LDAP client dials `authentik.int` instead
    # (`machines/aibox/sso.nix`, `machines/pi-room-sys`), and
    # `machines/services1/services/samba.nix` names it in a commented-out
    # `ldapsam:` line. docs/authentik-container-migration.md is the thing to read
    # before this one goes.
    ldap = "historical name for the authentik LDAP outpost; nothing in the tree serves it";
  };

  exempt = {
    # The one `*.int` vhost name on services1 that does not answer at services1.
    # nginx serves it (as an alias of `id.int`, the authentik portal) but the
    # appliance in `common/dns-records.nix` answers the name at 10.3.1.36, and
    # that is load-bearing: the switch logins' RADIUS and pi-room-sys's LDAP both
    # dial `ldaps://authentik.int.leighhack.org` (`machines/aibox/sso.nix`,
    # `machines/pi-room-sys/configuration.nix`), and the appliance's :443 is not
    # served through the front door.  Which answer *should* win is an open
    # decision — `docs/follow-ups.md` — but it is not a decision this
    # table gets to make by accident.
    authentik = "10.3.1.36, the appliance in common/dns-records.nix (RADIUS/LDAP dial it directly)";
  };
}

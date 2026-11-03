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
#   answeredBy names answered with *one* box's address, read as `via` out of
#              `common/hosts.nix`, which the door does not serve and cannot: the
#              thing dialling the name dials a **socket** on that box, not HTTP.
#              `legacy` cannot hold such a name (it answers at the door) and
#              `exempt` cannot (machines/gw/int-dns.nix asserts `names + exempt`
#              is exactly the door-served vhost names), and `fromHosts` cannot
#              because its label has to name a machine and a broker is not one
#
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
    # `mqtt` is **not** here. It moved out of this list on 2026-11-02, into the
    # `answeredBy` group below, and that move is the reason this list's 404-excuse
    # stops being true of the table as a whole: `legacy` answers with the front door,
    # and the one name here whose client dials a **socket** rather than a browser
    # therefore lost its socket the day the 2026-10-24 move re-pointed the door at
    # `gw` — nothing on this box listens on 1883, so the broker was not 404ed, it was
    # unanswered, and an MQTT client given no SYN reply sits inside its own timeout
    # rather than failing. Deleting the record was never the answer either: three
    # clients dial this name (Home Assistant, Frigate, zigbee2mqtt), all of them off-
    # flake runtime files this flake only bind-mounts. Read
    # docs/mqtt-int-legacy-answer-2026-11-02.md before moving any other name out of
    # here, and ask what dials a name before you decide which group it is in.
    #
    # ZigBee stays here because its client *is* a browser: `zigbee2mqtt.int` is
    # served at this box's nginx (`machines/services1/services/zigbee2mqtt.nix`), and
    # a 404 over HTTP is only an excuse for a name something loads into a tab.
    #
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
    ldap = "historical name for the authentik LDAP outpost; the outpost answers \"authentik.int\" (and 10.3.1.36) on 389/636, and nothing serves HTTPS on this name";
  };

  # A third answer, and the only shape it can be.
  #
  # A `*.int` name here can answer with the door, with a device's reservation, or —
  # this group — with the address of the box that runs what the caller reaches *through*
  # the name. That third answer exists because of one measurement, taken 2026-11-02
  # after the 2026-10-24 front-door move: `mqtt.int.leighhack.org` resolved at
  # 10.3.1.1, and nothing on that box listens on 1883 — `nc -z 10.3.1.1 1883` times
  # out in six seconds, while `10.3.1.20:1883` answers, and mosquitto logged each
  # probe as a protocol error at *its own* address. Nothing HTTP-ish was wrong
  # anywhere; the clients were Home Assistant's MQTT integration, Frigate's
  # notification channel and zigbee2mqtt, and the symptom was three weeks away from
  # DNS: Frigate cameras `Unavailable` because `FrigateCamera` is a
  # `FrigateMQTTEntity` whose `available()` needs an `available` push from a broker
  # nobody is connected to (docs/mqtt-int-legacy-answer-2026-11-02.md owns all of it,
  # including the fact that this name is **not** deletable — those three clients are
  # in runtime files this flake only bind-mounts, and a deleted record blacks them
  # out silently, which is what the parked 2026-10-22 follow-up had nearly done).
  #
  # Why it is a group and not one of the other lists: `legacy` answers a name at the
  # door, which is exactly what is wrong here, and `machines/gw/int-dns.nix` asserts
  # that `names ++ attrNames exempt` is *exactly* the names a vhost serves — putting
  # `mqtt` in `exempt` would claim the door serves MQTT, and no vhost does. `fromHosts`
  # is the near miss that forces a group: it answers with `hosts.<label>`, and `mqtt`
  # is a broker, not a machine. So this group is invisible to both drift guards, and
  # `machines/gw/lib/check-network-tables.nix` is what keeps it honest instead: a
  # `via` that is not a key of `common/hosts.nix`, a label that is also a device
  # record or a member of another group, and a name that renders as other than
  # exactly one line carrying its box's address are each refused.
  #
  # One address, IPv4 only, deliberately: the old `legacy` row handed this name the
  # door's two IPv6 halves, and `docs/router-and-dns.md`'s PARTIAL fault is a name
  # whose IPv6 half sends packets somewhere they go nowhere. A name answering with
  # A records only is the safe direction, and the MQTT clients here take the IPv4
  # route every time.
  answeredBy = {
    mqtt = {
      via = "services1";
      why = ''
        The MQTT broker is `machines/services1/services/mqtt.nix`'s podman
        `eclipse-mosquitto`, published on 0.0.0.0:1883 from services1. Three
        clients dial this name, all in off-flake runtime state: Home
        Assistant's MQTT config entry (apps1's `.storage/core.config_entries`),
        Frigate's `mqtt.host` (`/srv/frigate/config/config.yaml`), and
        zigbee2mqtt's `server:` (`/srv/zigbee2mqtt/configuration.yaml`).
        Home Assistant's is the entry that makes the fault read as a camera
        fault; the broker's journal is what shows the truth (`sudo journalctl
        -u podman-mqtt.service | grep -E "New client connected|disconnected"`).
      '';
    };
  };

  exempt = {
    # The one `*.int` vhost name that must not answer with the front door, and the reason
    # is a port the door does not proxy: `authentik.int` is the LDAPS endpoint (636) the
    # switch-adjacent world dials — aibox's sssd (`machines/aibox/sso.nix`) and the pi-room
    # netboot image (`machines/pi-room-sys/configuration.nix`, `ldap_tls_reqcert = demand`)
    # — and the RADIUS address the ProCurve switches have in NVRAM is the same host,
    # 10.3.1.36.
    #
    # Since the 2026-10-28 cutover that address is **not a separate box**: it is a second
    # address on services1 (see its `networking.nix`), so the same machine answers the name
    # for LDAPS/RADIUS and serves the `id.int` origin. Note what this exemption therefore
    # *costs*: because the name resolves to `.36` rather than to the door, **the door is
    # bypassed for `authentik.int` and no HTTPS is served on it** (measured: `curl
    # https://authentik.int.leighhack.org/` fails to connect, and it failed the same way
    # before the cutover, when `.36` was the appliance — the appliance served web on :9000
    # only, never :443). The alias in `common/front-door.nix` is what the door would serve
    # a request that arrived with that `Host`, which is not reachable through DNS. Which
    # answer should win, and whether the name should move to the door and leave the outpost
    # on the bare address, is the open item in `docs/follow-ups.md`.
    authentik = "10.3.1.36, a second address on services1 (RADIUS/LDAP dial it directly; no HTTPS on this name)";
  };
}

# Two routers — what CARP-style HA would take, and what it would not fix

Written 2026-10-27 in answer to "if we added another dual-NIC box, could we run two `gw`
instances with HA? OPNsense does it with CARP." **Nothing in this file is deployed, and
there is still exactly one router.** It is a decision record: what the option actually
consists of, what it costs in this flake, the four things it does not fix, and the
conditions that would make it worth building. Read `machines/gw/README.md` first if the
box itself is unfamiliar; `docs/router-and-dns.md` for which VLAN leaves by which uplink;
`docs/router-front-door-migration.md` for why one box holds the certificate.

**Verdict up front:** not now. The failure a second box protects against (dead hardware)
is already covered to within twenty minutes by the network's own timers, and the failure
CARP is famous for making seamless — connections surviving the switch — needs the one
piece this network never ran and Linux makes hardest. The gate on the whole idea is a
conversation with AAISP, not a NixOS module.

## The number that decides it before any protocol does

Two facts already in the tree set the recovery-time budget, and both are generous:

| Timer | Value | What it buys a dead router |
| --- | --- | --- |
| DHCP lease length (`machines/gw/dns.nix`'s `leaseTtl = 86400`) | 24 h, renewal ≈12 h | every host that already has a lease keeps it. `dhcp-authoritative` plus *the same generated tables* on the replacement means no client gets NAK'd off its address |
| RA router lifetime (`network-tables.nix` renders `ra-param=<vlan>,60,1200`) | 1200 s | every host with an IPv6 default keeps it for twenty minutes |

So a **manual** cutover that completes inside ~10 minutes costs the network nothing it
would not lose anyway: IPv4 keeps working off existing leases, IPv6 keeps its default, and
the only genuinely broken things are new leases and new IPv6 hosts. Compare that with what
a floating address actually buys — 1–3 s instead of minutes — and the gap is worth having
only against a *process* crash, which is not a job for a second box:

```bash
grep -n 'mkNeverGiveUpOverride' machines/gw/dns.nix      # dnsmasq: Restart=always, no rate limit
grep -n 'Restart = "always"'    machines/gw/http.nix     # nginx and the door
```

Those units already retry forever — `common/systemd.nix`'s house "never give up" policy,
`Restart=always` plus `startLimitIntervalSec=0` — so a dnsmasq that dies is restarted in
seconds on the same box. `nftables` is the deliberate exception — `Type=oneshot`, protected
at build time by `checkRuleset` instead — and it is not improved by a second box either.

The corollary matters when someone next proposes this: **the cold-spare route and the VRRP
route are not 10 minutes apart, they are 10 minutes apart *for the failure mode we have*.**

## The retired OPNsense box never ran CARP

This is the part that was assumed ("OPNsense does it with CARP") and is now measured. The
archived `config.xml` is on services1 at `~/tmp/opnsense-archive-20261017/` — a live
secret file (`docs/follow-ups.md` names five credentials in it), so re-measure it with
tag names only, never a value:

```bash
F=~/tmp/opnsense-archive-20261017/config.xml
grep -o -E "<mode>(carp|ipalias|proxyarp)</mode>" $F | sort | uniq -c
grep -c -o -E "<vhid>[0-9]+</vhid>" $F            # CARP needs a numeric VHID; 0 = no CARP
tr '<' '\n' < $F | grep -A30 -i '^hasync' | grep -E 'pfsync|sync|preempt|ppp'
```

What it says (2026-10-27):

* **Two virtual IPs, both `<mode>ipalias</mode>`, both `<vhid/>` empty.** `carp` appears
  once in the whole file, as an empty `<carp_depend_on/>`. No VHID means no CARP.
* **`<hasync>` is a form nobody filled in:** `pfsyncinterface/`, `pfsyncpeerip/`,
  `synchronizetoip/`, `username/`, `syncitems/` all empty. `disablepreempt` and
  `disconnectppps` are `0`. The only value is `pfsyncversion=1400`, which is **a default,
  not a configuration** — the wire-format selector the UI writes on save, and easy to
  mistake for evidence that state sync ran.
* One of the two ipaliases was `fd99:dead:beef:225d::1` on `lan` — the NPT66 source
  address the flake has already decided not to carry over (`machines/gw/README.md`'s
  table, `docs/router-and-dns.md`'s "the IPv6 half, which is the half that was lost").

`docs/follow-ups.md` said CARP/pf-sync were "disabled" there. That was right in direction
and soft in fact: **the HA feature was never configured at all.** Two consequences. Nobody
in this network's history has had the redundancy being asked for, so there is no regression
to restore and no inherited behaviour to match. And the one HA setting with a real bearing
on this network — `disconnectppps`, which makes a CARP backup tear down its PPP links and
redial when promoted — was **off**.

## What "OPNsense does it with CARP" actually is: four features, not one

From `opnsense/docs` (`manual/hacarp.rst`, `manual/firewall_vip.rst`) and FreeBSD
`carp(4)`/`pfsync(4)`:

| OPNsense feature | Mechanism | What it buys |
| --- | --- | --- |
| **CARP virtual IPs** | IP protocol **112**, adverts to `224.0.0.18` / `ff02::12` (unicast optional since 24.7), shared **virtual MAC `00:00:5e:00:01:XX`** derived from the VHID, `advbase` in seconds + `advskew` in **1/256 s**, SHA-1 authenticated adverts | the LAN address moves in ~1–3 s |
| **State table sync** ("Synchronize all states via") | **pfsync** pseudo-interface over pf's state table, multicast `224.0.0.240`, ≤1 s batching with `maxupd` collapse, `no-sync` rule keyword, versioned wire format. **No authentication on the protocol**; the man page says use a dedicated cable, or `syncpeer` + IPsec, because spoofing states that bypass the ruleset is trivial | **established connections and their NAT bindings survive** — this is the entire meaning of "seamless" |
| **Configuration synchronization** | XMLRPC to the peer, applied **on command**, or a cron `HA update and reconfigure backup` that *refuses to run unless every CARP interface is MASTER*. The doc's warning: "A backup node should never contain settings for Configuration synchronization" | both boxes hold the same rules |
| **Disconnect dialup interfaces** | a box configured as CARP backup disconnects PPP-type interfaces and reconnects them when it becomes master | the tunnel is not double-dialled |

Nothing on that list is "CARP". Two of the four are the interesting ones and they pull in
opposite directions: the valuable half (pfsync) is the half this network never ran and the
half whose own documentation demands a link nothing else is on; the fiddly half (XMLRPC,
never-touch-the-backup, the MASTER guard) is the half **this flake already does better** —
one set of tables in git and `just deploy` on the box that is being changed is config sync
without the footguns, and "the backup stays in a known-good state during maintenance" is
free when the backup is a commit you have not deployed.

## The Linux translation, measured against nixpkgs

Every claim below was checked against the nixpkgs tree on 2026-10-27 rather than
remembered.

| OPNsense part | nixpkgs | Notes |
| --- | --- | --- |
| CARP VIPs | **`services.keepalived`** — `nixos/modules/services/networking/keepalived/` (a directory module: `default.nix`, `vrrp-instance-options.nix`, `virtual-ip-options.nix`, `vrrp-script-options.nix`) | VRRP/VRRPv3, not CARP, and the same job. Options that exist: `enable`, `openFirewall`, `enableScriptSecurity`, `vrrpInstances.<n>.{interface,virtualRouterId,priority,noPreempt,state,trackInterfaces,trackScripts,unicastPeers,unicastSrcIp,useVmac,vmacInterface,vmacXmitBase,virtualIps,extraConfig}`, `vrrpScripts.<n>.{script,user,interval,timeout,weight,fall,rise,group,extraConfig}`, `extraGlobalDefs`, `extraConfig`, `secretFile`. Options that **do not** exist: `advertIntv` (goes in `extraConfig`) and any modelled `vrrp_sync_group` — "all interfaces move together", which OPNsense calls preempt, is hand-written `extraConfig`. `useVmac` is the equivalent of CARP's virtual MAC. `secretFile` is **envsubst interpolation, not protocol authentication** |
| CARP, literally | **`networking.ucarp`** — `nixos/modules/services/networking/ucarp.nix`, `pkgs/by-name/uc/ucarp` 1.5.2 | real CARP on the wire, SHA-1-authenticated adverts, adverts to `224.0.0.18` (read off `src/carp.c:784`), with `upscript`/`downscript` hooks. **Disqualified here: it is IPv4-only.** `ucarp.c` parses both `--srcip` and `--addr` with `inet_pton(AF_INET, …)`; the `AF_INET6` branches inherited from BSD's `carp.c` sit behind `#ifdef INET6` and are unreachable. Second finding: the module documents `advSkew` as "seconds" where `carp(4)` says 1/256 s. Choose it only to interoperate with a BSD peer — which here is nobody |
| pfsync | **`conntrack-tools`** (1.4.9) → `conntrackd`, **no NixOS module** | `Mode FTFW` (failover) over `Multicast` or `Unicast`, with `StartupResync`, `DisableExternalCache`/`CacheTimeout` for the external cache, `Helpers`/`Expectations`. Needs `nf_conntrack_events`, and both routers must run **identical NAT** — free here, since it would be one module on two machines. The hand-written unit, the boot-time resync and the "don't start until the VIP is up" ordering are all ours |
| Config sync | **already have it** | git + the tables + `just deploy <box>` |
| `disconnectppps` | trivial | gate `xl2tpd-aaisp` on a `vrrpScript`/`upscript`, or `systemctl stop` it in `downscript` |
| BGP-style alternatives | `pkgs/by-name/fr/frr`, `pkgs/by-name/bi/bird3`; `services/networking/frr.nix` exists | only relevant with an ASN and a provider that will announce for you; see the Anycast/BGP bullet below |

So the honest shape of "CARP HA on this flake" is **keepalived VRRP per VLAN + conntrackd
FTFW over a crosslink + `vrrpScripts` gating dnsmasq/nginx/xl2tpd + git for config**. VRRP
rather than CARP because ucarp cannot float an IPv6 address and this network's IPv6 is
load-bearing twice over (every VLAN's v6 egress is `ppp0`, and the public front door has a
AAAA). Losing CARP's HMAC on the adverts is a real but small loss either way: VRRPv2's
`auth_pass` is weak and VRRPv3 removed authentication, so the posture is pfsync's own
advice — this traffic is trusted by placement, not by crypto, and the crosslink should
carry nothing else.

## Four things it does not fix, and the mechanism for each

1. **The AAISP tunnel and the routed `/29`/`/48`.** CARP VIPs go on multi-access LAN
   interfaces; you cannot put CARP (or VRRP) on a ppp link. `disconnectppps` is OPNsense's
   entire answer — one dial at a time — and no amount of VIP makes AAISP route
   `81.187.195.17/29` and delegate the `/48` down whichever session appears. Since VLAN 225
   must egress the tunnel in *both* directions (`docs/router-and-dns.md`, "Which uplink
   each VLAN leaves by"), a failover router without the block is a router serving a LAN and
   a dead public door. **This is the gate.** Questions for AAISP, in the order they matter:
   can two L2TP sessions exist on one line at once; does the routed block and the PD
   follow a chosen or the first or a failed-over session; what username does a second
   session dial with; will they move the block on demand. Until those are answered, every
   design below protects the LAN and not the services.
2. **The IPv6 default gateway.** Neither CARP nor pfsync touches router advertisements, and
   this box's RA is dnsmasq's — which advertises from the advertiser's own address and
   cannot emit RFC 4191 route options, so the client's nexthop *is* the box. A promoted
   router's RAs do install a usable default when they arrive, but the dead one's entry ages
   out only at `ra-param`'s 1200 s, which is a transient double default on every host in
   the building. That is exactly the fault class `machines/gw/coexist.nix` was deleted for
   ("two RA advertisers … a network where every client picked a gateway at random",
   `machines/gw/README.md`), and the box's known silent failure — the RA timer that stops
   after a link flap and is invisible in `systemctl` — becomes a *permanent* thing to reason
   about across two boxes. And `machines/gw/networking.nix` already explains, in its own
   words, why every VLAN sets `IPv6AcceptRA = false`: a router that accepts an RA downstream
   "is a router that can install a default route via itself", and the old box's `enable-ra`
   plus a `UseGateway=true` "was a loop waiting for a second router to appear on a VLAN" —
   the second router is the thing this file is about.
3. **headscale and the tailnet.** `db.sqlite` is a single writer and
   `noise_private.key` *is* the tailnet's identity (`docs/router-front-door-migration.md`,
   "Moving headscale"): a fresh one unenrols every node. So it is copy-on-promote, with a
   staleness window equal to the sync interval, forever. And `gw` is now the network's only
   subnet router plus exit node, whose advertised routes are applied **at boot** and can be
   outranked by a hand-typed `tailscale up` — two clients advertising the same routes is
   headscale route churn, on top of the existing problem that this node registered as
   `gw-1` while a stale offline node owns `gw`.
4. **The certificate.** OPNsense gets this free because the cert rides inside the synced
   config. On Linux, two `security.acme` clients issuing the same SAN set burn Let's
   Encrypt's duplicate-certificate cap, and *both* can silently write nixpkgs' minica
   fallback — a healthy-looking nginx whose client-visible issuer is a local CA
   (`machines/gw/http.nix`, `front-door-cert-check.service`). So: renewal one-sided,
   `/var/lib/acme` synced, and the check still only proves anything on the box that ran it.

## The flake is not neutral about a second router either — and one claim here was wrong

Worth recording because it is the opposite of what I first assumed while writing this:

* **Nothing in this repo can tell the live router from a second box running `machines/gw`.**
  `machines/gw/lib/check-network-tables.nix` is *table*-scoped: it imports
  `common/vlans.nix`/`common/dns-records.nix` and asserts the VLAN's `.1` is inside its own
  block, outside every dynamic range, and not claimed by a reservation, and that
  `common/hosts.nix`'s addresses are covered by a reservation or a VLAN's `ipv4`. It never
  asks which machine is asking. `machines/gw/int-dns.nix` compares *this box's*
  `virtualHosts` to `common/int-dns.nix` — a second `gw` renders the same vhosts and passes.
  So the guard credited in `machines/gw/README.md` with refusing "a table where [`.1`] is
  not one of this box's own" **cannot refuse a duplicate router**; two boxes evaluating
  `machines/gw` produce byte-identical dnsmasq confs and both are correct. The only thing
  standing between this network and two DHCP servers is a cable. That is the same lesson the
  deleted co-existence mode taught, arriving from a new angle, and it means an HA build must
  add its own box-scoped assertion (a standby that renders dnsmasq/nginx/xl2tpd *disabled in
  the config*, plus an eval-time refusal if a standby ever holds both roles' addresses).
* **Three things keyed to the literal hostname** would mis-guard a second router named
  anything else: `_build-host-guard` and the `just check` flake-step skip (both
  `[ "$(hostname)" = "gw" ]`) and `just deploy`'s `case … gw) hkey=router` (all in the
  justfile). AGENTS.md rule 22's "don't build on the router" is a rule about a *role*; a
  second box in that role needs the same treatment, which is an argument for a shared
  predicate rather than a second string compare.
* **A standby still pays the router's taxes:** the age key at `/var/lib/sops-nix/key.txt`
  before its first evaluation (`just install-sops-key`, `common/sops.nix`), its own
  `hardware-configuration.nix` because the two `links` rules pin `router-lan` /
  `router-uplink` by MAC and a wrong MAC is a silent no-match, `max-jobs = 0` plus
  `infra.buildFarm.builderHost`, no GC, and its own `ssh_host_key` and known_hosts rows
  (`common/ssh-known-hosts.nix`, `just known-hosts`) because `10.3.1.1` would be answered by
  whichever box is holding the address.
* **Anycast/BGP is not on the table.** It needs an ASN and a provider that will announce
  for you, or a second transport; AAISP's L2TP model does not share a routed block between
  two live sessions. IPv4 ARP-anycast tricks on the LAN are a footgun whose failure arrives
  as the delayed, per-client, hours-later shape this network has already been bitten by
  twice.

## If someone builds it anyway, here is the order

Not a recommendation. Each step is independently useless until the last one is done, which
is the point of writing them down in dependency order.

1. **Get the AAISP answer first.** Everything after step 1 protects the LAN only.
2. **Close the state-backup gap regardless** — it is a prerequisite and it is worth doing on
   its own (see the ladder below).
3. A **crosslink** between the boxes carrying nothing else (pfsync's own advice), plus the
   VRF-free question of which VLANs must survive at all — 222 and 1337 have no IPv6 and
   could be left out of the VIP set to make the instance count smaller.
4. `keepalived` instances (225/226/227 first), `useVmac = true`, `trackScripts` asking the
   only questions worth asking — is `ppp0` up and is dnsmasq answering — with weights, i.e.
   BSD's CARP demotion.
5. `upscript`/`downscript` (or `vrrpScripts` + `notify`) starting and stopping dnsmasq,
   nginx and `xl2tpd-aaisp`, with both **disabled in the standby's config** so a runaway
   unit cannot make it live by accident.
6. `conntrackd` FTFW, then the honest acceptance test: **a long-lived TCP connection held
   open across a forced failover.** If it survives, conntrackd earned its keep; if it drops,
   you have VRRP and should know that before you ship it. Per AGENTS.md's *proving a
   component end to end is not proving the system*, drive it through the path users take — a
   client on VLAN 226 through the door and out the tunnel, not a `conntrack -K` on the box.
7. Then, and only then, the rehearsal: a full cold boot of the standby into the live role,
   timed, run twice, with the four router checks from `machines/gw/README.md` and
   `sudo tailscale debug prefs | jq .AdvertiseRoutes` (boot-only, remember) read back in the
   same shell invocation as the confirm. Bound every wait and poll, do not sleep through it
   (AGENTS.md rule 11, "Long jobs: poll, don't sleep").

## What I would do with a second box instead

In this order, all of it compatible with doing HA later:

1. **Back up the three directories that make `gw` itself**: `/var/lib/tailscale` (the node
   key — `machines/gw/README.md` names it as the last mutable state and it has **no
   backup**), `/var/lib/headscale` (the tailnet enrolment), `/var/lib/acme` (the cert, so a
   rebuilt box does not need a new issuance and does not spend the CA's rate limit). Borg
   exists and currently runs for services1, not for `gw`.
2. **Expire or rename the stale headscale node `gw`** (id 15, offline since 2026-04-13; the
   live box is `gw-1`) — otherwise the spare and the ACLs disagree about which machine is
   `gw`, which is a security-relevant disagreement, not a cosmetic one.
3. **Use the new box as the replacement, not the peer.** This box's LAN port negotiates
   **100 Mbit** (`machines/gw/README.md`, "Known-incomplete"), which is the ceiling on every
   byte the building sends. Swapping in better hardware and keeping the Supermicro as the
   cold spare gets the availability win out of the purchase with none of the dual-router
   risk.
4. **Shorten the paths that are actually single.** One switch port and one AAISP hub are
   unredundant and unredundant-able; trunks into both 2510Gs are available, and `gw` + the
   hub on one UPS removes the failure that starts the whole exercise.
5. **Make the outage visible before making it short.** `gw` is gatus-pinged and exports no
   metrics (`docs/follow-ups.md`), so a `ppp0` flap and a 100 Mbit negotiation are only
   observable as someone noticing. `network-status` on the box is the telemetry there is.
6. Ask AAISP the four questions even if never building HA — they are the same questions a
   *replacement* box hits on the day it matters.

## What this changed about what I thought

* "OPNsense does it with CARP" is true of OPNsense and false of this network: **the
  appliance ran neither CARP nor pfsync nor config sync** (`docs/follow-ups.md` said
  "disabled"; the file says "never configured", and `pfsyncversion=1400` is a default that
  looks like a decision).
* I initially claimed the network-tables guard would refuse to evaluate a standby that does
  not hold `10.3.1.1`. **It cannot, and no guard can** — the check never sees a box, only
  tables. The safety model for a second router has to be built, and it has to be box-scoped.
* **The part of CARP people forget is not the address, it is state**: without state sync —
  pfsync on FreeBSD, conntrackd on Linux — a failover resets every established connection,
  because the new master holds no NAT bindings for the return traffic. That is why a bare VRRP setup is
  "it comes back in a few seconds" and a CARP+pfsync setup is "nobody noticed", and why the
  acceptance test above is a held-open TCP connection and not a `ping`.

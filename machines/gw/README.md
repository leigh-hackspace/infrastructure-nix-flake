# gw — the router

The flake's replacement for the network's OPNsense box at `10.3.1.1`: DNS and DHCP
for every VLAN, the firewall that decides what may leave the building, the AAISP
tunnel that leaves it through, and the network's tailnet presence.

**The box is the network's router: a Supermicro X9SCL/X9SCM at `10.3.1.1`.** It was
installed 2026-10-17 in **co-existence mode** beside the OPNsense appliance, **cut over
on 2026-10-21**, and the appliance is retired. Co-existence mode itself was deleted on
2026-10-22 (`./coexist.nix`, its `hostAddr` substitution and the `!COEXIST.enable`
branches in four files): this box owns `.1` on every VLAN, advertises, leases and
leaves by the AAISP tunnel, and nothing in the tree can put it back into the shadow
mode. The table below is what that mode switched off, kept as the four checks that
prove the router is doing its job — they were the acceptance test at the cutover and
they are the ones to run after any change to `dns.nix`, `networking.nix` or
`uplink.nix`.

> **State as of 2026-10-22: deployed and live at `10.3.1.1`, measured.** The tunnel
dialled (`81.187.223.123`), DHCP/DNS/RA serve every VLAN (`ip -4 addr` shows `.1` on
225/226/227/1337 and `192.168.20.82` on the uplink; 70 live lines in
`/etc/dnsmasq.d/network.conf`, three of them `ra-`; the lease file holds 65 leases),
the policy routes and inbound NAT are installed. `docs/gw-cutover-attempt-2026-10-21.md`
is the post-mortem of the attempt and the findings from the live cutover.
`just switch-gw` defaults to `10.3.1.1`; the `10.3.1.254` seen below appears only in the
co-existence narrative and no longer answers.

```bash
# from services1, the whole loop: sync the mirror, build here, push to gw, confirm
just switch-gw

# what it will do, without touching a box
just check                                    # evaluates gw, checks the tables
nix build .#nixosConfigurations.gw.config.system.build.etc -o /tmp/gw-etc
                                              # renders dnsmasq's conf, the networkd
                                              # files, the nftables ruleset — read them
nixos-rebuild dry-build --flake .#gw          # builds the system, activates nothing

# on the box, which is the same thing with the steps visible
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.1
just switch                                   # `--flake .` resolves against hostname=gw
```

`gw` **is** in `just check-build` now, and that is the whole meaning of the box
existing: the recipe says "about to deploy", and this machine is.

## The box, as installed

Measured on the box, 2026-10-17, when it was installed over the Windows it arrived
running (that is what "erase Windows" meant, and it is why the disk layout below is
written down rather than assumed):

| | |
| --- | --- |
| Board | Supermicro X9SCL/X9SCM, BIOS 2.3a (2021-01-06), **UEFI** (`bootctl`: firmware supported, Secure Boot disabled in setup) |
| CPU / RAM | Xeon E3-1220 v2 (4 cores, no SMT) / 8 GB |
| Disk | Micron MTFDDAK256TDL 256 GB SATA (`/dev/disk/by-id/ata-Micron_MTFDDAK256TDL_2007267FBA4B`) |
| Layout | GPT: 1 GB ESP `NIXBOOT` (vfat), rest ext4 `NIXROOT`, no swap |
| Boot | systemd-boot from `common/base.nix`, entry `NixOS`, `root=fstab` (the same shape services1 and aibox boot with), `BootOrder` = Linux Boot Manager, its fallback, then the installer USB |
| NICs | two Intel e1000e: `00:25:90:d3:f0:ea` → `router-lan` (link up, **100 Mbit**, carrying the VLAN 225 trunk), `00:25:90:d3:f0:eb` → `router-uplink` (no carrier) |
| stateVersion | `26.05` — the release of the installer image the disk was laid out from, which is *not* the release this flake evaluates (26.11-pre). Do not "fix" it to another machine's value |

Installed from a NixOS 26.05 live USB (its own SSH endpoint, `root`/`password`), which
is gone now: the box runs the flake, its SSH is the fleet's (`leigh-admin` + the hop
key, root login `prohibit-password`), and the installer's own store was an overlay —
so the closure had to be copied onto the disk's store, not merely into the running
system's. That is the trap in any installer-based deploy here: **`nixos-install
--store-path` does not copy the closure to the target disk** (it registers it in the
chroot's view of the store, which on a live image is a tmpfs overlay), and the result
looks like a finished install until you reboot into a system with no bootloader.

## The four things a router must be seen to do

The mode this box used to run in switched four behaviours off, because each is a router
answering a request that belonged to `10.3.1.1`. They are now its whole job, and each is
still checkable in one command on the box — which is the only reason they are written
down twice, four days and one deletion later:

| It must | How that is done | How to prove it on the box |
| --- | --- | --- |
| own `.1` on every VLAN | `common/vlans.nix`'s `ipv4`, put on the interface by `./networking.nix`; `common/hosts.nix`'s `router` is that address, and `./lib/check-network-tables.nix` refuses a table where it is not one of this box's own | `ip -4 addr show scope global` → `10.3.1.1 10.3.2.1 10.3.14.1 10.3.22.1`; `getent hosts gw` answers `10.3.1.1` |
| hand out the leases | the generated `dhcp-range`/`dhcp-host`/`dhcp-boot` blocks in `/etc/dnsmasq.d/network.conf`, and `dhcp-authoritative` | `sudo wc -l /var/lib/dnsmasq/dnsmasq.leases` is non-zero and grows; `grep -c '^[^#]' /etc/dnsmasq.d/network.conf` is non-zero |
| announce the default gateway (RA) | `enable-ra = true` plus the generated `ra-only`/`ra-param` lines | `grep -c '^ra-' /etc/dnsmasq.d/network.conf` is non-zero, and see "the third way dnsmasq breaks the LAN silently" below — an RA timer that stopped is invisible in `systemctl` |
| advertise the tailnet routes it means to | `--advertise-exit-node` and `--advertise-routes` in ./uplink.nix | `sudo tailscale debug prefs \| jq .AdvertiseRoutes` — **and read "The tailnet's advertised routes only move at boot" below, because as of 2026-10-22 this one answers `null`** |

The lesson the mode taught and the deletion keeps: a shadow router is dangerous through
*behaviour*, not configuration. Two DHCP servers, two RA advertisers and two
advertised route sets all produce a network that works until a client picks the other
one, hours later, on a machine that is not this one. `./lib/check-network-tables.nix`
and the four commands above are what make that class of fault visible from the box
that causes it.

When something does look wrong, in the order that has been useful: `journalctl -u
xl2tpd-aaisp` (is the tunnel dialled?), `ip route show table 100` and `sudo ip route
show table 200` (are the two egresses installed?), `ip -6 route show default` (must be
`dev ppp0`, or there is no IPv6 anywhere), `networkctl status ppp0`, and
`systemctl restart dnsmasq` if the LAN link has flapped (the RA-timer trap below — a
dnsmasq that is `active` and has stopped advertising is the failure `systemctl` cannot
see).

## The rule this directory is written to

**Nothing here is invented.** Every address, VLAN id, lease, range and rule is
generated from the network's own tables — `common/vlans.nix`,
`common/dns-records.nix`, `common/hosts.nix`, `common/config.nix` — or it is a
`TODO` saying which fact is missing and where to read it. That is why the config is
worth having before the hardware: the parts that come from the tables are already as
correct as the tables are, and the parts that do not are marked, rather than being
plausible-looking numbers someone typed.

The corollary is the rule for editing: **if you want to change an address, a lease or
a range, change the table, not this directory.** `./lib/check-network-tables.nix` is
what makes that true rather than aspirational — it refuses a reservation naming a
VLAN that does not exist, two devices claiming one address, a dynamic range outside
its VLAN's block, a VLAN block the network's ACL does not cover, and a record with no
MAC and no client-id (which renders as no line at all, i.e. a reservation that
silently does not exist). It runs twice on purpose: as a module assertion, so no
evaluation of this machine succeeds with tables that disagree, and standalone via
`just check`, so the finding is a message naming the record rather than an eval error.

## Files

| File | Owns |
| --- | --- |
| `default.nix` | the import list, and the machine's name (`gw`, the name `common/hosts.nix` and every client's known_hosts entry use) |
| `configuration.nix` | system-level: packages (including the self-build trio `git`/`just`/`jq`), time, `stateVersion`, the daemon trust the fallback deploy needs. No NAS, deliberately |
| `hardware-configuration.nix` | the measured root FS, the ESP, and the two `links` rules that pin `router-lan`/`router-uplink` to the two e1000e MACs |
| `constants.nix` | `LAN_NIC`, `UPLINK_NIC`, `UPLINK_TUNNEL` — the three interface names the rest of the directory keys on |
| `networking.nix` | the VLAN netdevs, the addresses on them, the uplink NIC's DHCP |
| `dns.nix` | dnsmasq: the generated conf, the two generated hosts files (the device records and the `*.int` front door), the netboot variants |
| `http.nix` | nginx and the `int.leighhack.org` certificate: the `*.int` vhosts this box terminates, and the `dnsResolver` line the zone's own authority makes mandatory. Phase 1 of `docs/router-front-door-migration.md` — read that file before adding a vhost here |
| `int-dns.nix` | the guard over `common/int-dns.nix` read from *this box's* `virtualHosts`: subset while the answers live elsewhere, equal once `frontDoor` names `gw` (AGENTS.md rule 24) |
| `firewall.nix` | nftables: forward, input, NAT, and the IPv6 pair |
| `uplink.nix` | pppd's tunnel, the tailnet (headscale) client |
| `openvpn.nix` | the old box's two OpenVPN servers, **off by default** |
| `sops.nix` | the secrets this box needs: the AAISP PPP password, the tailnet's join key, and the shared env-file — for its `DO_AUTH_TOKEN` alone, because `http.nix` answers for the zone through DigitalOcean |
| `lib/network-tables.nix` | the renderers: tables → dnsmasq lines, nft set literals, device names |
| `lib/check-network-tables.nix` | the assertions over those tables (standalone, prints findings) |
| `lib/assert-network-tables.nix` | the three-line module wrapper that runs them at eval time |

## DNS: three generated files, nothing mutable

`docs/router-and-dns.md` is the network's DNS manual (what answers what, the exceptions,
DigitalOcean, the failure classes). What belongs here is the shape on this box:

| File | Answers | Written by |
| --- | --- | --- |
| `/etc/dnsmasq.d/network.conf` | DHCP ranges, reservations, RA ranges, netboot boot files | `./dns.nix` from `common/vlans.nix` + `common/dns-records.nix` |
| `/etc/dnsmasq-records.hosts` | every device's name, bare and `<name>.int.leighhack.org` | `./dns.nix` from `common/dns-records.nix` |
| `/etc/dnsmasq-int-vhosts.hosts` | every `*.int.leighhack.org` vhost, with the front door's three addresses | `./dns.nix` from `common/int-dns.nix` |

All three are store paths, all three are in `systemd.services.dnsmasq.restartTriggers`,
and dnsmasq reads the two hosts files **at startup** — which is why the trigger list is
not optional: before it, a switch that moved a device's address wrote the new file,
exited 0, and went on answering from the old one. The file that used to be here and was
*edited over ssh* by `crates/dns-sync`, `/var/lib/dnsmasq/names.hosts`, is the reason
`common/int-dns.nix` exists; see "Known-incomplete" for why it is still on disk.

```bash
dig +short @10.3.1.1 grafana.int.leighhack.org A AAAA   # bind is in this box's
getent ahosts grafana.int.leighhack.org                #   systemPackages; there is
sudo systemctl status dnsmasq --no-pager               #   no need for getent alone
sudo journalctl -u dnsmasq -n 30
```

(`docs/router-and-dns.md` used to say there is no `dig` on gw. There is — `bind` is
installed precisely so the resolver can be asked from the box.)

## What is proven, and from where

Proven means "read off the running old box", and the old box is
`ssh -i ~/.ssh/agent-hop-key root@10.3.1.1` (tcsh: `2>` fails, quote grep patterns
containing `<`/`>`). The artifacts that were read:

* **`/usr/local/etc/dnsmasq.conf`** — generated by OPNsense from its `<Interfaces>`
  XML. Source of: the per-VLAN `dhcp-range` lines and their tags, the reservations,
  the netboot `dhcp-match`/`dhcp-boot` variants, `local-ttl=1`, `strict-order`,
  `no-ident`, `dns-forward-max=5000`, `cache-size=10000`, the `wpad` suppression, the
  `*.int.leighhack.org` zone, the `ts.leighhack.org` conditional forward and its
  `rebind-domain-ok` exception, and the fact that dnsmasq answered on the VLANs, the
  OpenVPN TAPs and `tailscale0`.
* **`/conf/config.xml`** — the `<Interfaces>` section: which VLAN ids exist, which
  block each one is, which have IPv6. That is `common/vlans.nix`.
* **the running process list** — `/usr/local/sbin/nft -f /var/etc/nftables.conf`,
  which is why the flake's firewall is nftables and not the nixpkgs firewall module.
* **`uname`** — FreeBSD 15.1 on an mpc858 board. A fact about the *old* box: nothing
  here assumes the new one is the same board.

And, since 2026-10-17, proven **on the new box** — a different kind of proof, and
worth keeping separate from the list above because it says nothing about the old one:

* the disk, its UUIDs, the board, the two NIC MACs, UEFI, and the 100 Mbit LAN link,
  all in `hardware-configuration.nix`, from `nixos-generate-config` and `ip -br link`;
* the `.link` mechanism itself, validated by hand on the live box (`udevadm test` →
  `Set network interface name: router-uplink`, then the interface answering under that
  name) before it was trusted at boot — a wrong name here is a router with no
  interfaces, and the failure is silent until something needs one;
* the rendered conf, the networkd files and the ruleset, read back off the **running
  box** rather than off the build: `ip -4 addr` shows `.254` on all five VLANs, exactly
  one default route (`via 10.3.1.1 dev vlan225`), `nft list ruleset` loaded, dnsmasq
  answering on `:53`, zero leases handed out, tailnet joined and advertising nothing.

Everything not in those lists is inferred or marked. The old box's *shape* (one
generated file, tags from device names, nft from XML) is inherited; its *values* are
inherited only where they were read.

## Port 53, or why the first boot had no DNS

The box's first boot looked healthy. `systemctl is-system-running` said `running`, the
addresses were right, the ruleset was loaded — and the router answered no DNS at all,
because dnsmasq had exited 2 and was being retried forever:

```
dnsmasq: failed to create listening socket for port 53: Address already in use
```

`networking.useNetworkd` (which this box needs, for the VLAN netdevs) enables
systemd-resolved; resolved binds its stub to `127.0.0.53:53`; dnsmasq binds the
wildcard and filters by interface. The two cannot coexist, and a router that serves no
DNS has no failed unit to show for it. `dns.nix` now turns resolved off — and
openresolv's resolvconf with it, whose module asserts once `/etc/resolv.conf` is
managed elsewhere — and writes `/etc/resolv.conf` pointing at `127.0.0.1`. dnsmasq's
`no-resolv` is what stops that being a loop: the box resolves through itself, and
dnsmasq's upstreams are its own `server=` lines.

The tempting alternative was `bind-interfaces`, and it is the wrong way round twice
over: a VLAN device networkd has not created yet is a *fatal* error for dnsmasq with
that option (which is why `dns.nix` says not to set it), and a stub cache in front of
the network's authoritative answer is a second behaviour to reason about on the one box
whose answer *is* the network's.

## The second way dnsmasq refuses to start: duplicate dhcp-host lines

The co-existence mode this box shipped in **comments out** the DHCP blocks, so for
four days dnsmasq never parsed the `dhcp-host` lines and the box was healthy. The first
cutover un-commented them, and dnsmasq exited immediately:

```
dnsmasq: duplicate dhcp-host IP address 10.3.1.16 at line 30 of /etc/dnsmasq.d/network.conf
```

dnsmasq refuses **any two `dhcp-host` lines naming the same IPv4 address** — it does not
matter whether they differ by MAC, by client id or by hostname — and it refuses to
start rather than serving the one it manages to read. Multiple selectors on *one* line
are legal, and so is a second line carrying only an IPv6 address for the same host.
`lib/network-tables.nix`'s `hostLines` used to emit one line per *identifier*, so every
record with both a MAC and a client id produced two lines for one address, and the
assertion in `lib/check-network-tables.nix` counted identifiers rather than addresses
and passed.

That is one line per record with the selectors comma-joined, and the assertion now asks
that each record's IPv4 appear on exactly one `dhcp-host` line. The whole attempt — the
uplink that did work, the resolver that did not, the port forwards the flake does not
have — is written up in `docs/gw-cutover-attempt-2026-10-21.md`; read it before the next
cutover.

## The third way dnsmasq breaks the LAN silently: its RA timer stops on a link flap

Measured on the cut-over box, 2026-10-21, after the LAN trunk was re-cabled. It is the
failure that looks like "WiFi hangs and nothing else does":

* `router-lan` lost carrier at 17:14:30 and the VLANs followed. dnsmasq kept running and
  kept answering **solicited** RAs, but its **unsolicited** RA timer never restarted — the
  last periodic advertisement was 17:15:39 and the next one was a response to an `rdisc6`
  probe an hour later. `journalctl -u dnsmasq | grep 'RTR-ADVERT(vlan225)'` shows the
  cadence stopping dead; there is no error and no failed unit.
* The damage is delayed by exactly the router lifetime, so it does not look like the flap:
  clients keep the RA-learned default for `ra-param`'s 1200 s, then drop it. WiFi clients
  keep a public IPv6 address in `2001:8b0:1d14:226::/64` with **no default route**, so a
  dual-stack name (`id.leighhack.org`, `filestore.int.leighhack.org`) is tried over IPv6,
  hangs, and only then falls back to IPv4. On a phone that reads as "the internet is down".
* A host that *forwards* (services1, because of podman) does not take the RA default at all
  once it is forwarding, so it loses the ability to **reply** to the other VLANs. That is
  why the symptom includes services1 being unreachable over IPv6 from VLANs 226/227 while
  every wired client was fine — and it is repaired by the same restart, because the RS that
  the restart's burst provokes is what makes networkd install the default.

**The fix is `systemctl restart dnsmasq` on the router**, after any `router-lan` link
flap. There is no config change that prevents the timer from stopping; the flake has no
link-flap hook. Verify both halves rather than the symptom:

```bash
# the router is advertising again (should grow every ~20–60 s)
sudo journalctl -u dnsmasq --since '-70s' | grep -c 'RTR-ADVERT(vlan225) 2001'
# a host has a default and it is being refreshed, not counting down
ssh leigh-admin@10.3.1.20 'ip -6 route show default'
```

If the second line's `expires` keeps falling, the router is still not advertising; if it
resets towards 1200 s, it is. A one-off `rdisc6` makes the route appear and **hides** the
bug for 20 minutes, so do not treat "it works now" as proof. Longer term this is what a
dedicated RA daemon (radvd) would be immune to; it is listed in `docs/follow-ups.md`
rather than done here.

## "Only VLAN 225 has no internet": the tunnel's password, not the firewall

This is the one failure here that looks like a routing or nat bug and is a credential
bug. The policy it is being confused with — which VLAN leaves by which uplink, and why — is
`docs/router-and-dns.md`, "Which uplink each VLAN leaves by"; this section is only the
time the tunnel's *IPv4 path* looked wrong. The shape, measured on the cut-over box on
2026-10-21:

* **only VLAN 225 is affected, and only for IPv4.** Every other VLAN leaves through the
  hub (`mark 2` → table 200) and works; VLAN 225 is the one `./firewall.nix` marks `1`,
  i.e. the one sent into the AAISP tunnel. The router's *own* traffic works too, because
  its default route is the hub.
* **everything on the tunnel's side looks right**: `ppp0` holds `81.187.223.123`,
  `ip route show table 100` is `default dev ppp0`, `ip rule` has `fwmark 0x1 lookup
  100`, `routerNat` has `oifname "ppp0" … masquerade`, and `tcpdump -ni router-uplink
  'udp port 1701'` shows LCP echo request/reply in both directions. Nothing in this
  directory is misconfigured in a way that check can see.
* **and every packet inside the tunnel comes back rejected** in about 13 ms, from the
  LNS itself: `From 194.4.172.12 icmp_seq=1 Destination Net Unreachable`. Not a
  timeout — an ICMP unreachable for *every* destination, including the peer's own
  address (`ping -I ppp0 81.187.81.187`) and the LNS's (`ping -I ppp0 194.4.172.12`).
  A peer that cannot route to its own address is not doing a route lookup: it is saying
  the session has no service attached to it.
* **the dial log says it**: in `journalctl -u xl2tpd-aaisp`,
  `pppd: CHAP authentication succeeded: Authentication failure`. pppd prints the message
  from the CHAP *Success* packet after "succeeded", and AAISP's message is literally
  that: the link comes up and is given an address, but the session was not authorised,
  so nothing is routed through it.

The cause: **OPNsense stores the PPP password base64-encoded** in `/conf/config.xml`
(`<ppps><ppp><password>`), and `aaisp_ppp_password` was copied into
`secrets/secrets.yaml` verbatim on 2026-10-17 — so pppd dialled with the base64 *text*.
The tell is the length: 36 bytes in sops, 25 after `base64 -d` (compare hashes, not
contents — AGENTS.md). Decode it before it goes into sops, and note that `/run/secrets`
is rendered at activation, so the fix is `just switch` **and then** `systemctl restart
xl2tpd-aaisp`: it is the unit's `preStart` that copies the password into pppd's
`pap-secrets`/`chap-secrets`, and it only runs when the unit starts.

Two lessons worth more than the fix:

* **A tunnel that is up is not a tunnel that routes.** Both cutover attempts recorded
  "the tunnel came up, CHAP succeeded, table 100 got its default" as the uplink proven,
  and all of that was true while no packet ever crossed it
  (`docs/gw-cutover-attempt-2026-10-21.md`). The proof is `ping -I ppp0 1.1.1.1` from the
  router and a `curl` from a VLAN-225 host — traffic *through* the tunnel, not a look at
  its interface.
* **The failure mode is shaped like a firewall rule**, which is why it survived a
  cutover: the VLAN that has no internet is exactly the one the policy routing sends
  somewhere different. When one VLAN misbehaves and the others do not, the difference is
  the mark rule, so check what the marked path does (`ip route get 1.1.1.1 mark 1`) before
  reading the ruleset.

## Public IPv6 inbound is a rule, not a side effect

`curl -6 https://id.leighhack.org/` from the internet has exactly one thing holding
it up: one line in `./firewall.nix`. IPv4 inbound is easy to see because it is a
*translation* — `portForwards` names the public address, the port and the LAN box,
and `ct status dnat` then admits the packet. IPv6 has no translation, so the packet
from an internet client is already addressed to services1's own interface address
and nothing about it looks special; the v6 forward chain's `policy drop` therefore
refuses the entire public IPv6 internet, quietly and while every IPv4 test passes.

This was the state until 2026-10-21. The rule set that the retired box ran
(`pf.txt` in `~/tmp/opnsense-archive-20261017` on services1) had three lines the
flake had not reproduced:

```sh
pass in quick on l2tp0 reply-to (l2tp0 fe80::9e89:1eff:fe3f:0) inet6 proto tcp from any to <SERVICES1_IPV6> port = http
pass in quick on l2tp0 reply-to (l2tp0 fe80::9e89:1eff:fe3f:0) inet6 proto tcp from any to <SERVICES1_IPV6> port = https
pass in quick on l2tp0 reply-to (l2tp0 fe80::9e89:1eff:fe3f:0) inet6 proto tcp from any to <SERVICES1_IPV6> port = 8522
```

and the network publishes AAAAs for all of it: every `*.leighhack.org` vhost is a
CNAME to `nginx.leighhack.org`, which answers `2001:8b0:1d14:225::20` — so the
addresses the world dials are on the far side of this box, and were for four days
after the cutover.

The flake's version is `publicV6ForwardRules`, one rule, in the same table as the
forward chain so it cannot be installed apart from it, over the two addresses in
`constants.nix`'s `PUBLIC_SERVICES1_V6` (the published AAAA, and
`2001:8b0:1d14:225:d::1020` — the old box's `SERVICES1_IPV6`, still on services1
and still what headscale's DERP publishes as its IPv6). Read the rendered chain
rather than this comment:

```bash
sudo nft list chain ip6 router6 forward      # the iifname "ppp0" line, and its counters
```

Three things follow from this rule, and each is a decision rather than an
oversight:

* **80 and 443 only.**  The retired box's rule also let 8522 (GitLab SSH) in over
  v6; the 2026-10-21 post-cutover audit removed it to match the v4 policy (web plus
  tailscale only).  The v4 table's 3478, 50443 and 51820 were never allowed in over
  IPv6, so headscale's DERP has a published IPv6 address whose STUN socket is
  v4-only.  Opening them here would be a new listener on the network's public
  prefix, not a ported one.
* **`iifname "ppp0"`, not "not a VLAN"** — the hub's handoff has no public IPv6, and a
  rule without an input interface would let that segment pose as the internet.
* **The yunohost appliance has no v6 row** because `yunohost.leighhack.org` has no
  AAAA. Anything *new* that publishes one needs its address added to
  `PUBLIC_SERVICES1_V6`, and a vhost that publishes a AAAA on its own while this list
  does not know it is a vhost that works over IPv4 only, which is the failure mode
  this section exists to end.

The one thing this section cannot prove from inside the network: a LAN client
reaching those addresses matches `ip6 saddr 2001:8b0:1d14::/48` as well, so a LAN
`curl -6` passing says nothing about the new rule. Test it from a v6 address that is
not in `common/config.nix`'s ACL, or read the rule's packet counter after an
external probe.

## The tailnet's advertised routes only move at boot

`./uplink.nix` advertises the network's blocks and offers this box as an exit node,
and on the live box `sudo tailscale debug prefs | jq .AdvertiseRoutes` says `null`.
Both are true because those flags are `extraUpFlags`, and they reach the daemon only
when `tailscaled-autoconnect.service` runs `tailscale up` — **at boot**. A switch
changes the unit and not the node; and a `tailscale up` typed by hand on the box wins
from then on, because the daemon's prefs are what it reports and nothing re-asserts
them. That is what happened at the cutover: the unit timed out (`16:36:22 State =
NoState` → `16:37:53 Failed with result 'timeout'`, the login it wanted was the one
that could not resolve `81.187.195.17` until the inbound NAT existed), the join was
done by hand, and the box has advertised nothing since — it also registered as
`gw-1`, because the stale node `docs/follow-ups.md` names still owns `gw`.

So the check after a cutover, or after editing that line, is:

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.1 'sudo tailscale debug prefs | jq -c .AdvertiseRoutes'
```

and it wants a **boot**, not a switch. Before applying it, settle the overlap
`./uplink.nix` warns about in its own comment: services1 advertises
`10.3.0.0/16, 2001:8b0:1d14::/48, fd99:dead:beef:225::/64` imperatively (the
`tailscale set` line at the top of
`machines/services1/services/headscale.nix`, not the flake), which is unequal to the
`/48` this box advertises, and two advertisers of unequal prefixes give a route that
depends on which one headscale accepted first.

## Comparing the two routers, which is how to find what is wrong here

While both boxes were up there was a check better than reading this directory: ask the
same question of each and compare. That is how the 2026-10-17 bugs were found. **The old
box is retired now**, so the `.254` half below is history; the same instinct still works
against the public resolver (`@1.1.1.1`) and against the other LAN hosts.

```bash
# historical: gw in co-existence (.254) vs the then-live OPNsense (.1)
for n in services1 services1.int.leighhack.org aibox.int.leighhack.org 3d-blue.int.leighhack.org; do
  printf "%-36s old=%-14s new=%s\n" "$n" "$(dig +short @10.3.1.1 $n)" "$(dig +short @10.3.1.254 $n)"
done
```

That loop found two real bugs on 2026-10-17, and neither was visible any other way:

* **the generated records file answered short names and not FQDNs** — `local=` makes
  dnsmasq authoritative for a zone and does not complete a hosts entry, so every
  `*.int.leighhack.org` name would have gone NXDOMAIN at the cutover. Fixed in
  `lib/network-tables.nix`'s `recordsHosts` (one line per address, both names on it,
  the old box's shape), with two assertions in `lib/check-network-tables.nix` behind
  it: every record's FQDN must appear in the rendered text, and the line count must
  equal the address count, because dnsmasq reads only the first field of a hosts line
  as an address and the rest as names.
* **a records change never reached the running dnsmasq** — nixpkgs restarts it only
  for `/etc/hosts`, and dnsmasq reads `addn-hosts` at startup. Fixed with
  `restartTriggers` in `dns.nix`, verified by moving a record, switching, and watching
  dnsmasq restart and answer the new address.

Both had a healthy-looking system: `is-system-running` said `running`, the switch
exited 0, and the config read correctly. The bug was in the gap between the file and
the process, which is exactly what a second router answering the same question
exposes.

## How this box is deployed, and what the box itself is still for

`just switch-gw` from services1 (or aibox) does the whole loop: rsync this tree to the
router (the mirror), build gw's system **on the build host**, push it with
`nixos-rebuild switch --flake .#gw --target-host leigh-admin@10.3.1.1 --elevate=sudo`,
then `sudo nixos-confirm` over ssh in the same shell invocation (AGENTS.md's GOLDEN
RULE — `nixos-confirm` on services1 confirms nothing, because it confirms the local
system).

That is the primary path because the router hosts a Rust crate since 2026-10-21
(`./network-status.nix`), and a build started here is aibox's work rather than this
box's — `just switch-gw` is still the form that keeps the evaluation off the router
too, so it stays build-shaped and refuses to run on gw itself.

The box still keeps the checkout at `~/Projects/infrastructure-nix-flake`, with
`git`/`just`/`jq` in its own `environment.systemPackages` (not from
`common/tools.nix`, which this machine must not import because tools.nix and nas.nix
arrive as a pair), and it reaches GitHub for the flake inputs over its own route out.
That is for reading the tree, for `just check` (eval and format only), and for the
emergency case where the push path is the broken thing: `sudo nixos-rebuild switch
--flake .` locally still works, but it now compiles the dashboard crate on the router,
so it is a fallback rather than the way.

Four things the box needs for that checkout to work at all, and none of them are in
the flake. Three were set during the install; the fourth is a habit:

1. `sudo git config --global --add safe.directory /home/leigh-admin/Projects/infrastructure-nix-flake`
   — root's config, because `sudo` here sets `HOME=/root`, so the same command run
   without `sudo` writes a file nix never reads and the emergency local build dies with
   "repository path … is not owned by current user". Every machine in the fleet has it;
   it is one of the imperative pre-flight checks in `docs/network-upgrade-runbook.md`.
2. `/var/lib/sops-nix/key.txt` — the network's age key, `root:nixbld 0440`, from
   services1. Needed for a local evaluation/build of this box's config; when the deploy
   is pushed from services1, *that* box's copy is what the evaluation uses.
3. `~/.ssh/agent-hop-key` (+ `.pub`) for `leigh-admin` — the fleet's shared hop key, so
   the box can `git fetch infra`, a remote named for services1's checkout and the only
   one it can reach without a GitHub key of its own (`origin` still points at GitHub,
   like everywhere else).
4. The tree is a **mirror of the admin's working tree**, kept with `just sync-gw`
   (`rsync -a --delete`, excluding `target/` and `node_modules/`). It is not a clone to
   edit on the box: edit here, sync there. Discovering a divergence at the cutover is
   the failure this rule exists to prevent.

The push path has its own two gotchas, both hit for real:

* **`--elevate=sudo` is not the default.** This nixpkgs ships `nixos-rebuild-ng`, which
  does not elevate remotely where the old shell script did, so without the flag the
  deploy builds, copies, and dies with `Permission denied` creating the profile
  symlink. Running the *client* under `sudo` does not substitute for it — the elevator
  is a separate decision from the client's uid.
* **`--target-host` uses plain ssh, and the default identity is not authorised here.**
  It needs `NIX_SSHOPTS="-i ~/.ssh/agent-hop-key -o BatchMode=yes"` (which
  `just switch-gw` sets); and the copy needs **daemon trust**, which is what
  `nix.settings.trusted-users` in `configuration.nix` grants. Without the latter the
  push dies with "lacks a signature by a trusted key". Neither exists on a fresh
  install, so the first generation has to be laid down by `nixos-install` from the
  installer, or streamed into the disk's store as root (`nix-store --export … | sudo
  nix-store --import`) — which is how gen 2 arrived.

Two more things about this box that its files do not mention, both worth knowing before
you read `/run/secrets` and wonder:

* `common/sops.nix` declares two *shared* secrets (`immich_oidc_client_secret`,
  `status_dashboard_lan_token`) for every machine that imports it, so they render on
  the router too. Harmless (root:secrets, 0440) and the price of one shared secrets
  module; `machines/gw/sops.nix` owns only the router's own.
* The tailnet node joined as **`gw-1`**, not `gw`: headscale already had a node called
  `gw` (id 15, 100.64.0.11, registered 2025-08-25, offline since 2026-04-13;
  `docs/follow-ups.md` has it). Rename or expire that one before the cutover, or the
  tailnet has two `gw`s and the ACLs and the humans reading them mean different
  machines.

## Netboot is a two-box path, and the split is easy to get wrong

The generated `dhcp-boot` lines name `/etc/tftp/undionly.kpxe` and
`/etc/tftp/ipxe.efi` with `common/hosts.nix`'s `aibox` address as the server.
**aibox is the TFTP server** — `machines/aibox/netboot.nix` runs
`in.tftpd -l -a 10.3.1.32:69 … /etc/tftp`, and those paths are its `/etc/tftp`, not
this box's. The router serves the DHCP answer only; it has no `/etc/tftp` and no
`enable-tftp`, which is deliberate (the bootfile name is what the client asks the
named server for, so the path and the server address are one fact and
`./lib/network-tables.nix` keeps them together). Moving netboot to the router means
moving the three files *and* the server address, not one of them.

## Facts that were read off the old box, and what is still open

The old box was reachable until 2026-10-17, and its config, pf rules, interface
state and generated PPP files were archived under
`~/tmp/opnsense-archive-20261017` on services1.  What that settled:

| Fact | Where it went | State |
| --- | --- | --- |
| LAN/uplink MACs | `hardware-configuration.nix` `links` rules | **read and proven** on the live box; which physical port is which is still an assumption (re-check `ip -br link` when the handoff cable moves) |
| The uplink takes its address from the hub by DHCP | `networking.nix` `10-uplink` | **read**: old `enc0` was `192.168.20.41` on the hub's handoff, and networkd now requests the same |
| L2TP endpoint, username, password | `constants.nix`, `sops.nix`, `uplink.nix` | **read**: `194.4.172.12`, `lhc1@a`, and the password is now `aaisp_ppp_password` in `secrets/secrets.yaml` |
| Tunnel MTU and keep-alive | `uplink.nix` pppd options | **read**: `ifconfig l2tp0` was MTU 1460, and the old keep-alive was `10 60` |
| The peer's IPv6 default | `constants.nix` `UPLINK_TUNNEL_PEER6` | **read**: `fe80::9e89:1eff:fe3f:0` from `netstat -rn -f inet6` |
| AAISP's dial script | replaced by `xl2tpd` | **not needed**: `xl2tpd` is the daemon pppd's `pppol2tp` plugin expects, and it dials the same L2TP endpoint |
| The tailnet's `ts.leighhack.org` resolver address | `dns.nix` `server` line | **read**: `100.100.100.100`, exactly as the placeholder guessed |
| The routes the old box advertised to the tailnet | `uplink.nix` `--advertise-routes` | **open**: the new list is the network's aggregate blocks; headscale still has to approve them for this node, **and the live node advertises nothing at all** — the flags are boot-only and the cutover's join was typed by hand (see "The tailnet's advertised routes only move at boot") |
| Public IPv6 inbound | `firewall.nix` `publicV6ForwardRules`, `constants.nix` `PUBLIC_SERVICES1_V6` | **read and implemented 2026-10-21**: the old box's three `pass in on l2tp0 inet6 … to <SERVICES1_IPV6>` rules (:80, :443, :8522), which the flake did not have for its first four days. The same day's post-cutover audit cut 8522, so it is **80/443 only** now — see "Public IPv6 inbound is a rule, not a side effect" |
| The tailnet's own address block | `common/config.nix` `TAILNET_V4`/`TAILNET_V6` | **read**: `<alias name="TAILSCALE">` was `100.64.0.0/10, fd7a:115c:a1e0::/48`, and headscale allocates from that /10 — three copies of the fact here said /16 until 2026-10-21 |
| The upstream resolvers | `dns.nix` `server` | **read**: `<dnsserver>` was 9.9.9.9, 8.8.8.8, 1.1.1.1, 8.8.4.4. `dnsallowoverride` (letting the uplink's DHCP hand back resolvers) is deliberately **not** inherited |
| Interface offload knobs | nothing | **read but not ported**: `<Interfaces><settings>` disabled checksum, segmentation and large-receive offloading and the VLAN hardware filter. Those are FreeBSD-driver-era settings; Linux's defaults for `e1000e` want them on, so this is recorded rather than copied — if the tunnel ever misbehaves on TCP, this is a difference from the box that worked |
| VLAN 50 (`MILL`) | absent from `common/vlans.nix` | **read and deliberately absent**: the old box defined `vlan0.50` on `em0` and put no interface on it (the uplink was untagged `em3`, `192.168.20.0/24`). If the mill's handoff is ever re-tagged, this box needs a netdev for it |
| Config backup (`sftp` to `nas2`, 60 copies) | nothing | **not ported, and the gap is now almost nothing**: the config is this tree and, since 2026-10-22, so is its DNS — both `addn-hosts` files are store paths generated from `common/dns-records.nix` and `common/int-dns.nix`, so a rebuilt box answers exactly what this one answers. The only mutable state left is `/var/lib/tailscale` (the node key), which has no backup, and the `backups@nas2` id_ed25519 that job used is still authorised on the NAS for a box that no longer exists |
| The OpenVPN server | `openvpn.nix` | **read but not migrated**: one TUN server `MembersVPN`, UDP 1194, pool `10.3.254.0/24`, push `10.3.0.0/16`, Authentik LDAP auth (`Members`, LDAPS to 10.3.1.36:636 with a `cn=pfsense-gw` bind DN), certificates in the old XML. It was reachable from the internet: `pass in on l2tp0 proto {tcp,udp} from any to any port = openvpn` |
| How the network's IPv6 prefix arrives | `networking.nix` `10-tunnel` and `50-vlan226/227` | **read**: DHCPv6-PD, `/48` hint, VLAN id as `SubnetId`; networkd now requests it and assigns the per-VLAN /64s |
| The old box's firewall | `firewall.nix` | **read**: `pfctl -sr`/`-sn`/`-sa`; the per-VLAN `route-to` split is copied as fwmark policy routing |

Still open for the cutover itself: the old box's **OpenVPN server** and its
**port-forward/round-robin NAT rules** are not reproduced here (the tunnel and the
LAN routing are).  If those are still in use, they need a follow-up before the old
box goes away for good.  `network-status` was the last OPNsense-shaped tool; it was
ported **and moved onto this box** on 2026-10-21 (`./network-status.nix`, a
DynamicUser reading the local `/proc` and `/sys`).  The last tool that still spoke to
this box from outside it — `crates/dns-sync`, ssh'ing in from services1 to append DNS
records — was deleted on 2026-10-22; `common/int-dns.nix` is that feature now.

## Known-incomplete, by name

* **This box terminates TLS for `*.int` and nothing is sent to it yet** (`http.nix`,
  phase 1 of `docs/router-front-door-migration.md`). `common/int-dns.nix`'s `frontDoor`
  still says `services1`, so every `*.int` answer is still 10.3.1.20 and the two vhosts
  here are reachable only by name resolution you force: `curl --resolve
  bar.int.leighhack.org:443:10.3.1.1 https://bar.int.leighhack.org/`. What that curl
  proves is the point — cert, ACL, listener and the `./int-dns.nix` guard, tested on the
  box that will own them before any client depends on them — and the one part of it that
  cannot be tested without waiting is *renewal*.
  `security.acme.defaults.dnsResolver` in `http.nix` is what stops lego asking **this
  box's own dnsmasq** whether the `_acme-challenge.int` TXT it wrote to DigitalOcean has
  arrived: dnsmasq is authoritative for `int.leighhack.org`
  (`local = "/int.leighhack.org/"`, `no-resolv`), so its answer is a negative that never
  stops being true, and without that line the certificate would renew by never renewing
  — silently, on a 60-day timer, at 04:00. If `acme-int.leighhack.org.service` is ever
  red, read that comment before reading the config again.

* **`/var/lib/dnsmasq/names.hosts` is dead weight, deliberately, for one deploy.** It
  is no longer in dnsmasq's `addn-hosts` list (`./dns.nix`), so nothing reads it, but
  tmpfiles still creates it — because a generation built before 2026-10-22 names it
  and `system.autoRollback` can put that generation back within five minutes of a
  switch, and dnsmasq **exits** on a missing `addn-hosts` file. Deleting the file in
  the deploy that made it dead therefore turns a rollback into a router with no DNS.
  The follow-up (file + tmpfiles rule, together, in a later deploy) is in
  `docs/follow-ups.md`. Its contents are in git: every line of it is now generated
  from `common/dns-records.nix` and `common/int-dns.nix`, and the 2026-10-21 snapshot
  is `/var/lib/dns-sync-backups/` here and on services1.
* **The LAN port is doing 100 Mbit, not gigabit** (measured at install). Cable, PHY, or
  the switch port it lands on. Irrelevant while it is a shadow; it is the ceiling on
  every byte the network sends once it is not, so read the switch port's own config
  before blaming the NIC.
* **`mitigations=off` is deliberately absent** from this box's kernel command line,
  unlike the other three machines. The tradeoff is `common/base.nix`'s to make, and the
  cutover is the moment to make it for this box;
  `hardware-configuration.nix` says why the installer did not decide it.
* **OpenVPN is off** (`infra.gw.openvpn.enable = false`). An OpenVPN listener with
  guessed certificates on the network's public address is worse than none; the
  firewall rule for its port lives in `openvpn.nix` so it cannot outlive the service.
  The facts needed to turn it on are in the table above. **This is a service the
  network had and no longer has**: the old box let members dial `vpn.leighhack.org`
  from the internet over TCP *and* UDP 1194 and auth'd them against authentik's
  `Members` group, and nothing here does. Reviving it needs a decision plus three
  secrets that were in the old box's XML (the CA and its key, the server cert and its
  key, and the LDAP bind password), so it is `docs/follow-ups.md`'s problem, not a
  flag to flip.
* **The `nftables` unit gets no never-give-up override**, unlike every other unit here,
  because it is `Type=oneshot` and systemd does not restart those. What protects the
  ruleset instead: nixpkgs validates it against a user-mode kernel at *build* time
  (`networking.nftables.checkRuleset`, which has already caught two real mistakes in
  this file), and `reloadIfChanged` re-applies it on switch.
* **The VLAN match form is unverified.** `firewall.nix` emits both `vlan id N` and
  `iifname "vlanN"` per VLAN because they match at different points in the kernel's
  VLAN path and which one is redundant is a question about the real box. Read
  `nft list ruleset` and the counters on it, then delete the half that never matches.
  The box now *loads* the ruleset, so those counters are readable — and they are all
  zero for forwarded traffic while co-existence is on, which is expected and is not
  evidence of anything.
* **`system.stateVersion` is `26.05`** — the installer's release, which is a measurement
  of this disk rather than a copy of another machine's value. It is the one number here
  meant to disagree with the flake's nixpkgs (26.11-pre).

## Re-installing this box, or installing the next one

The procedure that actually produced the current box, including the steps that are easy
to skip and one reboot away from an unbootable machine:

1. Boot the installer, get onto the network, and **partition the target disk** (the old
   OS is not "going away": `wipefs -a`, then GPT, 1 GB ESP (`esp`, fat32), the rest
   ext4, `mkfs.vfat -F 32`, `mkfs.ext4`, mounted at `/mnt` and `/mnt/boot`). If the
   disk was partitioned earlier in this session, `partprobe` and `udevadm settle`
   before `mkfs` — the node for the new partition may not exist yet.
2. `nixos-generate-config --root /mnt`, then put the flake's
   `hardware-configuration.nix` in place of the generated one. The generator cannot
   know the two `links` rules (`matchConfig.MACAddress` + `linkConfig.Name`); the MAC
   in `ruleConfig` is a silent no-match, which is how this file was first written.
3. Lay the closure on the **disk**, not in the live system: `nix copy --to /mnt`, or
   `nixos-install --store-path <toplevel>` — which builds nothing and copies nothing,
   it only registers and links. Then
   `nix-env --store /mnt -p /mnt/nix/var/nix/profiles/system --set <toplevel>`.
   **The trap:** a live image's `/nix/store` is an overlay, so a copy that never named
   `/mnt` leaves 12 GB of store that the reboot deletes, and every command in between
   looks like it worked.
4. Put the sops key at `/mnt/var/lib/sops-nix/key.txt` (`root:nixbld 0440`; the gid is
   `nixbld`'s because `common/sops.nix` reads the file during *evaluation*) and `touch
   /mnt/etc/NIXOS`.
5. Set the root password **on the disk**
   (`nixos-enter -c '/nix/var/nix/profiles/system/sw/bin/chpasswd'`), because the flake
   sets no `passwordFile` and a fresh NixOS has no console login. Do it while the
   installer is still reachable.
6. Install the bootloader, because step 3 skipped the step that does it:
   `NIXOS_INSTALL_BOOTLOADER=1 nixos-enter -c '<install-systemd-boot.sh>'`, taking the
   script's path from the new system's own `INSTALL_BOOTLOADER` export in
   `<toplevel>/bin/switch-to-configuration` — running the *new system's* copy is what
   keeps the installed version the one `switch` will use. Verify the ESP has
   `loader/entries/` and `EFI/systemd/` before rebooting.
7. Check the firmware will boot it: `BootOrder` under `/sys/firmware/efi/efivars`. The
   installer's USB entry is a live one and disappears, but a `Windows Boot Manager`
   entry pointing at a disk you just erased does not, and it outranks the new entry
   until something fixes the order.
8. Then unmount, reboot, and **confirm on the way back in**: `sudo nixos-confirm`, and
   `readlink -f /run/current-system /nix/var/nix/profiles/system-good` equal. Note that
   on a fresh install there is no `system-good` yet, so the rollback timer disarms
   itself (`No generation is marked as good yet`) instead of saving you: the first
   confirm creates the safety net rather than checking it, and it has to be in the same
   invocation as the switch that needed it.
9. Then the four imperative facts from the section above, after which
   `just switch-gw` is the way forward.

## Do not build on this box

This is a router: 4 cores, 8 GB, and the DNS/DHCP answer for every VLAN. It now **delegates
its builds to aibox** (`infra.buildFarm.builderHost` in `./default.nix`, same as
services1) and runs its own daemon with **`nix.settings.max-jobs = 0`**, so there is nothing to
fall back to: a build that aibox cannot take fails instead of quietly compiling here.
Check it with `nix config show | grep -E 'max-jobs|build-withers'` and `cat
/etc/nix/machines`. The garbage-collection half of `common/build-farm.nix` is switched
off for this box (`infra.buildFarm.gc = false`) — a router should not be rewriting its
own store on a timer, and `system-good` is the rollback profile's own GC root.

That makes the *accident* harmless; it does not make building here correct. Evaluation
still costs this box CPU, and the deliberate path is still build-there-and-push (see
the next paragraph).

`nix develop` used to be a build: the one devShell this box had pinned
`CRANE.wasmBindgenCli`, which is compiled with cargo, so entering it here compiled on the
gateway. On 2026-10-21 a `just check` on this box ran `just fmt-check` → `nix
develop` and put four `rustc` processes on the gateway. Two changes keep them off:
`devShells.default` builds nothing (the wasm toolchain lives in `devShells.wasm`, which
nothing on this box has a reason to enter), and the justfile keeps the recipes apart —
`just check` and `just fmt` evaluate/format only (`nix flake check --no-build`,
`nix fmt`), and the build-shaped recipes (`test`, `clippy`, `check-build`,
`build-frontend`, `filestore-*`, `switch-gui-test`, `build-remote`, **and
`switch-gw`**) refuse here via `_build-host-guard`. Run those from aibox or services1.
`just switch` still exists here, and after this generation it can no longer compile
anything without aibox: the box hosts a Rust crate (`./network-status.nix`), that crate
is built on aibox whether the switch was started here or on services1, and the closure
is copied back. `just switch-gw` from services1/aibox remains the way — it is the same
build with the *evaluation* off the router too, and `sync-gw` keeps this checkout as a
mirror rather than as the source. If aibox is genuinely down and the push path is
unavailable, the escape hatch is a per-command `--option max-jobs N` on a box where
that is exactly the risk you have chosen; nobody has tested that it is honoured for a
trusted client, so do not discover that during an outage. Mechanism and the incident:
`docs/nix-dev-workflow.md`, "Never build on the router".


Do not read the Nix to find out what the router will do; read what the Nix produced.

```
nix build .#nixosConfigurations.gw.config.system.build.etc -o /tmp/gw-etc
less /tmp/gw-etc/etc/dnsmasq.d/network.conf      # the generated leases and ranges
less /tmp/gw-etc/etc/systemd/network/            # netdevs, addresses, the uplink
cat /nix/store/…-nftables-rules                  # the ruleset (find it: nix build -o … .#…etc)
nix build .#nixosConfigurations.gw.config.system.build.toplevel -o /tmp/gw-top   # slow
```

On a running box: `nft list ruleset` (read-only), `systemctl status pppd-aaisp-tunnel`,
`journalctl -u dnsmasq -n 50`, `dig @10.3.1.1 services1.int.leighhack.org` — and if you
are on a generation still in co-existence, `@10.3.1.254` is where *it* answers; comparing
the two answers is the fastest way to tell whether the tables and the live box agree —
`ip -4 addr` and `ip route show default` (one default route, via the old router),
`sudo tailscale status --json | jq .Self.AllowedIPs`,
`tail /var/lib/dnsmasq/dnsmasq.leases` (empty, and it has to stay empty until the
cutover), `ip -d link show vlan225`.

## Security notes

**What the internet can reach (audited 2026-10-21, after the cutover).** Read the
live rules rather than this list — `sudo nft list chain ip routerNat dstnat`,
`ip6 router6 forward`, `ip routerIn input` — but the intended set is:

| From the internet | Port | What |
| --- | --- | --- |
| 81.187.195.17 | tcp 80, 443 | services1: nginx, every `*.leighhack.org` vhost |
| 81.187.195.17 | udp 3478 | headscale DERP STUN → services1:3479 (tailscale) |
| 81.187.195.18 | tcp 80, 443 | the yunohost appliance |
| services1's public IPv6 | tcp 80, 443 | the same vhosts, no NAT |
| gw itself | udp 41641 | tailscale's direct data plane (both families) |

The retired box's 8522 (GitLab SSH), 50443 (matrix/livekit) and 51820 (WireGuard to
the NAS) forwards were removed on 2026-10-21: not web, not tailscale, and 8522/50443
had no listener left on services1.  Nothing else on the gateway answers from the
WAN: DNS and DHCP are `iifname "vlan*"`/`tailscale0`, SSH is source-gated by the
ACL below, `network-status` is `10.3.1.20` only, and the L2TP daemon's 1701 listener
is not in any input rule (the tunnel's own replies are `ct established`).

**The `*.int` front door is `iifname`-scoped for the same reason, and that is not a
slip** (`frontDoorRules`, 2026-10-24). nginx now listens on 80/443 here, and the rules
that admit traffic to it name the *interfaces* dnsmasq serves — `vlan*` and `tailscale0`
— rather than `ip saddr ${aclV4}` the way the SSH rule two lines below does. The ACL
contains five public ranges (next paragraph), this box has public addresses of its own
(`2001:8b0:1d14:225::1`, and the tunnel's IPv4), and the only thing that keeps `*.int` a
private namespace today is that **no public address answers one**. An ACL-shaped rule
would have put the internal vhosts on the internet for five lucky sources; the interface
rule puts them where the names resolve, which is the invariant worth stating: *if you can
resolve it here, you can reach the door here*. It adds no DNAT row — the public 80/443
still translate to services1 — and this box could not answer a public name if asked,
because `machines/gw/http.nix` holds a certificate that does not cover one.

**The ACL is a source exception, and it is wider than the port policy.**
`common/config.nix`'s `LOCAL_NETWORK` contains five *public* ranges —
`51.148.168.145/32`, `217.155.231.1/32`, `2a02:8010:6680::/48`,
`2a0a:ef40:154a::/48`, `2001:4860:7::/48` — alongside the LAN and tailnet, and
both the input SSH rule and the forward chain trust the whole ACL.  So from those
addresses port 22 is open and forwarded traffic on *any* port is admitted, which is
not "80/443 + tailscale".  On top of that, `sshd -T` reports
`passwordauthentication yes` (NixOS' default), so anyone in the ACL who reaches 22 can
attempt a password even though the fleet's access is key-based.  Moving those admins
to the tailnet (their nodes are already there) or splitting `LOCAL_NETWORK` into
"service clients" and "routable admins", and disabling password/KbdInteractive auth,
are the next security decisions; neither is changed here because both can lock
someone out.  `docs/follow-ups.md` carries them as open items.

**ICMP echo is answered on every interface**, v4 and v6, so the router is pingable
from the internet. That is not a port and is needed for diagnosing a dead uplink
from a laptop with no DNS; it is noted because it is the one other WAN response.

* **One router, by construction.** There is no mode that can make this box a second
  authoritative DHCP server or a second RA advertiser: `common/vlans.nix` gives it `.1`
  on every VLAN, `./lib/check-network-tables.nix` refuses to evaluate if any VLAN's own
  address falls inside a dynamic range or is claimed by a reservation, and the
  assertions run as a module as well as via `just check`. The failure that class is an
  availability attack on the whole network launched by a well-meant deploy, which is why
  it is an eval error rather than a paragraph of advice. What is left of it is
  behavioural, not declarative: two routers cannot be told apart by reading one of them,
  so the four checks near the top of this file are run on the box.
* `--dev-user`-style test hooks do not exist here, and nothing in this directory
  listens on the WAN: dnsmasq's `interface=` list is the VLANs only (the old box's
  generated file listed every interface OPNsense knew, WAN included, because it was
  generated wholesale from `<Interfaces>`).
* The old box's admin UI on 443 is **not** opened. There is no web UI to reach; ssh
  from the network's ACL is the way in.
* The input chains are `policy drop`. Secrets are read by PID 1 from
  `/run/secrets/<name>` and referenced by path — no secret appears in this directory,
  in a rendered unit, or in a transcript.
* The firewall's boundary is `common/config.nix`'s `LOCAL_NETWORK`, the same table the
  network's nginx servers read. The per-VLAN rules are anti-spoofing, not a boundary,
  and are documented as such in `firewall.nix`.

# gw — the router

The flake's replacement for the network's OPNsense box at `10.3.1.1`: DNS and DHCP
for every VLAN, the firewall that decides what may leave the building, the AAISP
tunnel that leaves it through, and the network's tailnet presence.

**The box is the network's router: a Supermicro X9SCL/X9SCM at `10.3.1.1`.** It was
installed 2026-10-17 in **co-existence mode** beside the OPNsense appliance and **cut
over on 2026-10-21**, so `./coexist.nix` is `enable = false` and the appliance is
retired. Co-existence mode still exists in that file (it is both the switch back and the
mode the first four days ran in) — read it before editing anything here, because the four
ways a shadow router can become the real one by accident are the four things the cutover
flipped.

> **State as of 2026-10-21 evening: deployed and live at `10.3.1.1`.** The tunnel dialled
> (`81.187.223.123`), DHCP/DNS/RA serve every VLAN, the policy routes and inbound NAT are
> installed. `docs/gw-cutover-attempt-2026-10-21.md` is the post-mortem of the attempt,
> the findings from the live cutover, and what is still open. `just switch-gw` defaults to
> `10.3.1.1`; the `10.3.1.254` seen below appears only in the co-existence narrative and
> no longer answers.

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

## Co-existence mode, and the four ways to be the router by accident

`./coexist.nix` is on. It rewrites exactly one thing in the tables — the host part of
every VLAN's gateway address, `.1` → `.254` — and switches off four behaviours, each
of which is a router answering a request that belongs to `10.3.1.1`:

| It must not… | How that is done | How to prove it on the box |
| --- | --- | --- |
| take `.1` on any VLAN | `coexist.nix`'s `hostAddr`, applied to the VLAN list every renderer reads | `ip -4 addr show` — every address ends `.254`; `getent hosts gw` answers `10.3.1.254`, not the `hosts.nix` value |
| hand out leases | `no-dhcp-interface=` for all five VLAN devices, and the generated `dhcp-range`/`dhcp-host`/`dhcp-boot` blocks emitted **commented** | `grep -c '^[^#]' /etc/dnsmasq.d/network.conf` is `0`; `/var/lib/dnsmasq/dnsmasq.leases` is empty |
| announce a default gateway (RA) | `enable-ra = false`; the `ra-only`/`ra-param` lines are in the conf, commented | `grep -c '^# *ra-' /etc/dnsmasq.d/network.conf` is non-zero, `^[^#]` is zero |
| advertise tailnet routes | `--advertise-routes`/`--advertise-exit-node` are not passed at all | `sudo tailscale status --json \| jq .Self.AllowedIPs` — two `/128`s and nothing else |

Commented rather than omitted, deliberately: whoever runs the cutover has to be able
to read what they are about to enable, and an absent block looks exactly like a bug in
the generator.

The mode is one substitution rather than a branch in four files because `coexist.nix`
exports the VLAN list and `dns.nix`, `networking.nix`, `firewall.nix` and
`uplink.nix` all read *that* instead of `common/vlans.nix`. A file that read the raw
table would take `.1` on one VLAN and `.254` on the next, and `check-network-tables.nix`
cannot see that from here.

**The cutover is `enable = false` in `./coexist.nix`** — and not before the old box's
facts have been read (the table below), the AAISP secret exists, and the old router
has actually been taken out of service. Two DHCP servers and two RA advertisers on one
VLAN do not fail loudly: clients get two leases for one address and pick a gateway at
random, hours later, on a machine that is not this one.

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
| `default.nix` | the import list, and the machine's name (`gw`, the name `common/hosts.nix` and dns-sync's known-hosts file use) |
| `coexist.nix` | **the mode this box runs in**: the host part it takes, the router it is a shadow of, the VLAN list everyone else reads. `enable = false` is the cutover |
| `configuration.nix` | system-level: packages (including the self-build trio `git`/`just`/`jq`), time, `stateVersion`, the daemon trust the fallback deploy needs. No NAS, deliberately |
| `hardware-configuration.nix` | the measured root FS, the ESP, and the two `links` rules that pin `router-lan`/`router-uplink` to the two e1000e MACs |
| `constants.nix` | `LAN_NIC`, `UPLINK_NIC`, `UPLINK_TUNNEL` — the three interface names the rest of the directory keys on |
| `networking.nix` | the VLAN netdevs, the addresses on them, the uplink NIC's DHCP |
| `dns.nix` | dnsmasq: the generated conf, the records file, the netboot variants |
| `firewall.nix` | nftables: forward, input, NAT, and the IPv6 pair |
| `uplink.nix` | pppd's tunnel, the tailnet (headscale) client |
| `openvpn.nix` | the old box's two OpenVPN servers, **off by default** |
| `sops.nix` | the secrets this box needs. One of them does not exist yet, which is why the tunnel's is declared only when there is a tunnel |
| `lib/network-tables.nix` | the renderers: tables → dnsmasq lines, nft set literals, device names |
| `lib/check-network-tables.nix` | the assertions over those tables (standalone, prints findings) |
| `lib/assert-network-tables.nix` | the three-line module wrapper that runs them at eval time |

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
bug. The shape, measured on the cut-over box on 2026-10-21:

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
(`./network-status.nix`), and the box has no remote builder: a local
`nixos-rebuild` here would compile Rust on the box that answers DNS/DHCP for every
VLAN. `just switch-gw` is therefore build-shaped and refuses to run on gw itself.

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
| The routes the old box advertised to the tailnet | `uplink.nix` `--advertise-routes` | **open**: the new list is the network's aggregate blocks; headscale still has to approve them for this node |
| The OpenVPN server | `openvpn.nix` | **read but not migrated**: one TUN server `MembersVPN`, UDP 1194, pool `10.3.254.0/24`, push `10.3.0.0/16`, Authentik LDAP auth, certificates in the old XML |
| How the network's IPv6 prefix arrives | `networking.nix` `10-tunnel` and `50-vlan226/227` | **read**: DHCPv6-PD, `/48` hint, VLAN id as `SubnetId`; networkd now requests it and assigns the per-VLAN /64s |
| The old box's firewall | `firewall.nix` | **read**: `pfctl -sr`/`-sn`/`-sa`; the per-VLAN `route-to` split is copied as fwmark policy routing |

Still open for the cutover itself: the old box's **OpenVPN server** and its
**port-forward/round-robin NAT rules** are not reproduced here (the tunnel and the
LAN routing are).  If those are still in use, they need a follow-up before the old
box goes away for good.  `network-status` was the last OPNsense-shaped tool; it was
ported **and moved onto this box** on 2026-10-21 (`./network-status.nix`, a
DynamicUser reading the local `/proc` and `/sys`), and dns-sync was ported the same
day and maintains `/var/lib/dnsmasq/names.hosts`.

## Known-incomplete, by name

* **dns-sync writes `/var/lib/dnsmasq/names.hosts`** (ported 2026-10-21, the same day
  as the cutover). It connects as `leigh-admin` with `sudo -n`, replaces the file
  atomically and reloads dnsmasq; the contract, and why a record is one line per
  address, is at the top of `crates/dns-sync/src/main.rs`, with the manual fallback in
  `docs/router-and-dns.md`. Two properties of this file are worth knowing before
  touching it: it is **mutable state the flake never rewrites** (tmpfiles creates it
  empty, and that is the whole of the flake's involvement), and it is **shared with
  records dns-sync does not own** — the cameras, APs, Pi Room boards, `nginx.int`,
  `services1`, and `authentik.int` at the appliance. So deleting it does not "let
  dns-sync rebuild it": dns-sync refuses to run against a file it cannot read, and the
  network is NXDOMAIN in the meantime. Snapshot before any hand edit; the 2026-10-21
  snapshot is `/var/lib/dns-sync-backups/` here and on services1.
* **Nothing here is the network's router yet** — that *was* `./coexist.nix`, and it was
  flipped on 2026-10-21. `.1` addresses, DHCP, RAs, the AAISP tunnel and the policy
  routing are all live now; the things that had to be proven first (the tunnel, the
  fwmark rules, the port forwards, the delegated IPv6 prefixes) are in
  `docs/gw-cutover-attempt-2026-10-21.md`, together with the three that were not.
  The checks when something looks wrong: `journalctl -u xl2tpd-aaisp`, `ip route show
  table 100`, `ip -6 route`, `networkctl status ppp0`, and `systemctl restart dnsmasq`
  if the LAN link has flapped (the RA-timer trap above).
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
  The facts needed to turn it on are in the table above.
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

This is a router: 4 cores, 8 GB, and the DNS/DHCP answer for every VLAN. It has **no
remote builder** — only services1 sets `infra.buildFarm.builderHost`, so `nix config
show | grep builders` here is empty and a build runs locally. That is deliberate.

`nix develop` is a build: the devShell pins `CRANE.wasmBindgenCli`, which is compiled
with cargo. On 2026-10-21 a `just check` on this box ran `just fmt-check` → `nix
develop` and put four `rustc` processes on the gateway. The justfile now keeps them
off: `just check` and `just fmt` evaluate/format only (`nix flake check --no-build`,
`nix fmt`), and the build-shaped recipes (`test`, `clippy`, `check-build`,
`build-frontend`, `filestore-*`, `switch-gui-test`, `build-remote`, **and
`switch-gw`**) refuse here via `_build-host-guard`. Run those from aibox or services1.
`just switch` still exists here for the emergency local build, but since the box
hosts a Rust crate (`./network-status.nix`) it now compiles that crate on the router:
use it only when the push path is unavailable, and prefer `just switch-gw` from
services1/aibox. Mechanism and the incident: `docs/nix-dev-workflow.md`, "Never build
on the router".


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

* **The mode is the security property as much as the networking one:** while
  `./coexist.nix` is on, this box refuses DHCP by interface name, sends no router
  advertisement, takes no `.1`, and advertises no tailnet routes. Those are not
  conveniences — a second authoritative DHCP server and a second RA advertiser are an
  availability attack on the whole network launched by an well-meant deploy, and the
  assertions in `./lib/check-network-tables.nix` are what stop an edit to `hostAddr`
  (`.1`, or an address inside a lease range) from evaluating at all.
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

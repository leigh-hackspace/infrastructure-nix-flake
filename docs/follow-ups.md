# Follow-ups — the live backlog

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps a short list of the ones
that an agent can trip over or fix in passing; this file keeps each item with the
context that makes it actionable. **Add an item here (with a date and what blocks
it) rather than growing `AGENTS.md`** — that is what keeps the rules file a rules
file.

Narrative of work already finished: `docs/work-log-2026-10.md`.

---

## Nix / build

- **`nix flake check` is not warning-free: two findings, both reachable now**
  (noticed 2026-10-14 while checking the `gw` machine; re-read on 2026-10-15 during
  the network upgrade). Checking `nixosConfigurations.services2` emits
  `evaluation warning: Grafana passwords will be stored as plaintext in the Nix
  store! Use file provider or an env-var instead.`, which by rule 9 is a finding:
  either the Grafana provisioner moves to the file provider /
  `GF_SECURITY_ADMIN_PASSWORD__FILE`-style env, or the warning is the documented
  exception. **The blocker that used to hide it is gone at the 2026-10-14 lock:**
  `nix flake check` no longer dies on
  `error: path '…-sops-secret-<name>.drv' is not valid` — it reaches all five
  configurations (`services1`, `services2`, `aibox`, `pi-room-sys`, `gw`) and exits
  0, so both warnings are now visible from one command and both are findings.
  (Note `nix flake check` is not a cheap eval gate: it *builds* `packages.strata`,
  `packages.strata-cuda`, `packages.whisper-ws` and the devShell — ~10 minutes on a
  lock bump.)

- **Any tracked edit anywhere re-hashes pi-room-sys's and aibox's system
  derivations** (measured 2026-10-18 while proving a `common/crane.nix` change was
  drv-neutral). `services1`, `services2` and `gw`'s toplevel `drvPath`s are stable
  across an unrelated edit; `pi-room-sys`'s is not — one HTML comment appended to
  `docs/follow-ups.md` produced a third distinct drvPath, and the moving parts are
  `append-initrd-secrets` and `boot.json`. aibox inherits it through
  `machines/aibox/netboot.nix`'s `netboot.ipxe`, which names the pi-room-sys
  closure, so a doc edit re-instantiates aibox's `/etc` too. Nothing rebuilds
  *expensively* (the kernel and the squashfs store keep their paths), but it makes
  "identical drvPath" unusable as a proof for those two machines and it dirties
  every deploy's diff. Suspect a path literal that drags a whole-directory store
  path into the initrd (`./leigh-logo.png`, `./ssh_host_ed25519_key` in
  `machines/pi-room-sys/configuration.nix` are the candidates; a single-file literal
  should be content-keyed, so the real culprit is probably how one of them is
  consumed). Blocked on: reading the generated `append-initrd-secrets` drv, which is
  not on disk — instantiate it with `nix build --no-link --keep-going` on the
  pi-room-sys toplevel and inspect, rather than guessing from the module.

- **~17 GB of fat `-source` copies that only `nix store gc` can reclaim**
  (2026-10-18). Five 2.9 GB copies of the pre-`crates/` working tree, from the
  `builtins.getFlake "/abs/path"` evals of 2026-10-11/12, are each named by 2–6 old
  `.drv` files, so `nix store delete` refuses them; the three 7.7 GB copies from
  2026-10-18 were unreferenced and went away with 22.6 GB freed. Two things to know
  before repeating it: the unreferenced ones still refused as `still alive` until
  `--ignore-liveness` was used (no roots, no referrers, no process naming them —
  unexplained, so check `nix store verify --all` if it recurs), and a client whose
  environ names a path roots it, so assemble path strings inside the script
  (`docs/nix-dev-workflow.md`). What is left is a `nix store gc` decision on this
  box, which is a maintenance call rather than a fix: 113 GB used, 93 GB free.

- **aibox is the builder now; prove the hop and watch its disk** (2026-10-17, with
  the move off services2). Two things are open, neither of them a switch:

  1. **The delegated build itself.** `common/build-farm.nix`'s hop was last proven
     against services2 on 2026-10-12 (a delegated build and a `--max-jobs 0` build,
     no SIGKILL sweep). The key, `sshUser = "leigh-admin"` and the
     `trusted-users` entry are box-agnostic, and aibox already has
     `~/.ssh/agent-hop-key`, but run `just build-remote` once from services1 and
     read the builder name in the log — a silent fallback to local is the failure
     mode the rule about polling exists for.
  2. **aibox's root and the parked model data.** aibox measured 82 % full on
     2026-10-15, while services2 had 211 GB free; the builder is the box that now
     grows `/nix/store`. Strata's data is deliberately outside the store and is
     **not** removed by disabling the service: the IQ2_XS GGUFs under
     `/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF` (~68 GB) and the
     pack/MTP under `/home/leigh-admin/Strata`. Deleting them is a manual decision
     — do not automate it from the flake — but until then the builder's headroom
     is smaller than the role assumes. `common/build-farm.nix`'s GC policy is what
     keeps it from being a surprise.

- **`packages.strata-cuda` emits four upstream deprecation warnings (open, one
  line).** `checking derivation packages.x86_64-linux.strata-cuda` prints
  `stdenv.isLinux` / `isAarch64` / `isDarwin` "is deprecated, use
  stdenv.hostPlatform.…". They are not ours — nothing in this tree says `stdenv.is*`
  (`grep -rn 'stdenv\.is' --include=*.nix` is empty) — they come from the CUDA-enabled
  package set `pkgsCuda` (`nixpkgs` with `cudaSupport = true`, `flake.nix:295`) and
  the `cudaPackages_13_0` overlays. Rule 9 says a warning is a finding, so either
  they get an exception here with a pointer to the upstream file that still uses the
  old attribute, or they go when nixpkgs' CUDA glue does. `packages.strata` (the
  HIP one on aibox) is clean.

- **`crates/target/` is not excluded from `workspaceSrc` (open, one line).**
  `common/crane.nix` filters the four SPA directories and `whisper-ws` out of
  `workspaceSrc` and nothing else, and `lib.cleanSourceFilter` does not read
  `.gitignore`, so an evaluation that reaches `crates/` as a **path literal**
  copies 3.1 GB of cargo artifacts into the store while holding the nix big lock —
  which is what wedged the box on 2026-10-14 (mechanics:
  `docs/nix-dev-workflow.md`). Harmless through the flake, since `self` arrives
  git-filtered, so this is hardening rather than a fault: adding `target` to that
  filter makes the mistake impossible to repeat. (`crates/whisper-ws/` is its own
  cargo workspace, so it writes a second `target/` one level down; that one is
  already excluded, as a side effect of `whisper-ws` being a non-member.)

- **The `deploy` account is dead weight and should go.** `useradd`ed on 2026-10-11
  as a workaround for the build-user bug (build as a uid that owns nothing), with
  its own copy of the repo and the private flake inputs under `/home/deploy`. The
  bug is fixed (`common/nixbld.nix`, deployed 2026-10-12), so nothing needs it, and
  it is not declarative — a switch will not recreate it. Remove it
  (`userdel -r deploy`) unless something turns out to depend on it. Do **not**
  deploy through it: builds as `leigh-admin` are safe and that path is the one
  `just switch` takes. (Since 2026-10-15 the "private flake inputs" half of why it
  existed is gone entirely: the flake has no `~/Projects` inputs to copy.)

- **services2 has no swap partition** (services1 has 18 GB). Fine at the current
  load, worth matching before it runs anything heavier than builds. **Update
  2026-10-15:** it has 15 GB of *swap file* (`/var/swap/services2` — the
  `DEVICE=/var/swap/services2` in its units is swapon), so the partition is absent
  but the swap is not; what is true is that a full three-machine build pushed the
  box to 0 MiB free and ~6 GiB swap while strata held 24-26 GiB resident. It survived;
  the margin is not there twice.

- **Declare root's `safe.directory` for the repo, or stop depending on it** (found
  2026-10-15, first run of `docs/network-upgrade-runbook.md`). `sudo nixos-rebuild
  switch --flake .` opens the checkout as **root**, and root's HOME is `/root`
  (`sudo`'s `secure_path`/`env_reset`), so git's ownership check fires: `repository
  path '…infrastructure-nix-flake' is not owned by current user (libgit2 error code
  = 7)`. services1 and services2 have `/root/.gitconfig` with
  `safe.directory = /home/leigh-admin/Projects/infrastructure-nix-flake`; **aibox
  did not**, and its first switch of the run died on it in the first second. That is
  imperative, undeclared, per-host state under the single most-used command in the
  repo — one `nix.settings`-adjacent line (or `programs.git.safeDirectories` for
  root, or `sudo` keeping `HOME`) makes three boxes agree. Until then Phase 0 checks
  it, because the failure looks like a permissions bug in the deploy rather than a
  missing config file on one box.

- **aibox's netboot export makes every switch of that box exit 4** (found
  2026-10-15). `/exports/netboot-squashfs` is a `fileSystems` **bind mount of a
  store path** (`machines/aibox/netboot.nix`), so the source changes on every
  nixpkgs bump; the mount unit is boot-generated from `/etc/fstab` (nothing in the
  toplevel's `etc/systemd/system`), so activation's `systemctl restart` of it fails
  *after* `/run/current-system` has flipped and after nginx/sshd/immich/strata have
  restarted. The workaround is the `just switch-netboot` dance (`umount -f -l` +
  `mount -a`, then switch again) and it is now written into the runbook; the fix is
  to stop the export path from moving — export a directory that holds a **symlink**
  to the current image (or `linkAs` the squashfs into a stable name), so the bind
  mount is created once and never restarted. Until then: aibox's switch is expected
  to fail once, and the second failure is the stop condition.

  It no longer panics clients when it does fail: the menu boots
  `init=/nix/store/.netboot-closure/init` and each image carries that symlink to
  its own closure (2026-10-21), so a client that mounts the *previous* image boots
  the previous generation instead of dying in `initrd-find-nixos-closure`. A
  refresh that leaves the export *unmounted* still stops clients from booting at
  all, so the dance stays in the runbook.

## Machines / access

- **services2 cannot hop out.** `machines/services2/README.md` used to claim the
  shared machine-hop key was installed there; it is not. Only `authorized_keys` is
  (by `common/users.nix`), so you can hop *in* to 10.3.1.21 but it cannot ssh to
  services1, and it has no GitHub key of its own. Fetch there with `ssh -A` (agent
  forwarding — `AGENTS.md`, "SSH between machines") rather than copying a private
  key around. Install `~/.ssh/agent-hop-key` deliberately if it should hop out;
  never make it a side effect of a deploy.

- **`machines/pi-room-sys/ssh_host_ed25519_key` is a private host key in plain
  git** (moved in with the tree on 2026-10-15; it was committed in
  `leigh-hackspace/pi-room-sys` before that, so this is not a new leak — it is a
  new place to fix it). `netboot.nix` puts it in the netboot initrd as
  `boot.initrd.network.ssh.hostKeys`, so it is needed at *build* time: it should
  become a sops secret rendered through `config.sopsSecretText`
  (`common/sops.nix`, which already exists for exactly this) and then be
  **rotated**, because anyone who can PXE can read the initrd and the same file is
  in git history in two repositories. While it is a plain file it is also why
  `nix flake check` cannot run without a checkout of `machines/pi-room-sys/`:
  the path literal is copied at evaluation. Related, same file: `emergencyAccess =
  true` gives an unauthenticated root shell in the initrd, which is a lab
  convenience on the Pi Room VLAN and still worth knowing about
  (`machines/pi-room-sys/README.md`).

## DNS / monitoring

- **Two public status vhosts have no DNS record** (found 2026-10-12 while
  sanity-checking after the `nixbld` fix).
  `machines/services1/services/status.nix` defines the SSO vhosts
  `status.leighhack.org` and `aibox.status.leighhack.org`, and
  `common/monitoring-endpoints.nix` monitors `https://status.leighhack.org`, but
  both names are **NXDOMAIN** in the DigitalOcean zone — so they are unreachable and
  that gatus check cannot be passing. The `*.int` dashboards are fine
  (`services1-status.int` / `services2-status.int` / `aibox-status.int` /
  `network-info.int` all serve 200). dns-sync only manages the `*.int` CNAMEs;
  public names are added by hand in DO DNS and these two never were. Either add the
  records (public apps CNAME → `nginx.leighhack.org`) or drop the vhosts and the
  endpoint.

- **Three gatus checks point at names that have never existed** (found 2026-10-15
  while taking the upgrade's endpoint baseline). `common/monitoring-endpoints.nix`
  monitors `http://3d-1.int.leighhack.org`, `3d-2` and `3d-3`; `getent` says
  NXDOMAIN and `common/dns-records.nix` has no such devices — the printers are
  `3d-blue` (10.3.14.62) and `3d-lime` (10.3.14.61), and there is no third. So three
  of the network's ~30 checks have been red since they were written, which is worse
  than no check: it trains the reader to ignore the red. Either the names are the
  old ones and the file is stale, or a third printer was planned and never bought.
  Fix the endpoints to the two real names (and say why a check is allowed to be red
  if it ever is again).

- **Nothing monitors the borg server** (`nas2.int:3022`), so a dead `borg-server`
  container only shows up as a failed nightly job — and one nightly run has already
  failed with no recorded reason. Details: `docs/nas-and-borg.md`.

- **Nothing measures services1 from outside itself** (opened 2026-10-17 by the move
  that put gatus, Prometheus and Grafana on services1). Every check that used to
  watch services1 — gatus's ping of 10.3.1.20, and the https checks that go through
  its nginx — now runs on the box they watch, so a dead services1 goes down with its
  own monitor and reports nothing, and the dashboards are unreachable at the same
  moment they stop being updated.  The other boxes are still measured from here
  (gatus pings services2/aibox/router/NAS/switches, Prometheus scrapes their
  exporters), so the gap is one-sided.  The fix is a second, minimal monitor on a
  box that is not services1 — gatus is a small Go binary and aibox has the
  headroom — or a cron/one-shot on aibox that curls services1 and alerts to Slack.
  Do not "fix" it by keeping the services1 self-ping in
  `common/monitoring-endpoints.nix`: a check on the box that runs it can only ever
  catch a hung nginx, and it was removed on 2026-10-17 for that reason.

## Router (gw)

- **`CONFIG.LOCAL_DOMAIN` exists; most of the network does not use it yet** (added
  2026-10-17). The router needed the zone name in one place because three uses of it
  have to agree exactly (`domain=`, `local=`, and the suffix on every generated
  record — see `machines/gw/dns.nix`), and the failure of them disagreeing is a silent
  empty answer for the whole zone. `common/nginx/`, `common/monitoring-endpoints.nix`
  and the machines' `networking.search` still spell `int.leighhack.org` out. Converting
  them is mechanical and worth a pass of its own, not a ride-along with a router
  change.

- **dns-sync is OPNsense-shaped and the cutover has to move it** — **done
  2026-10-21**, same evening as the cutover, so `just dns-sync-check|sync|prune` work
  against `gw`: it connects as `leigh-admin` + `sudo -n`, reads and writes
  `/var/lib/dnsmasq/names.hosts` (the file `machines/gw/dns.nix` creates empty for
  exactly this), reloads dnsmasq with SIGHUP, and the host-key pin is `gw`'s own. The
  port, and the two facts worth keeping from it, are at the top of
  `crates/dns-sync/src/main.rs` and in `docs/router-and-dns.md`; what it was replacing
  is in `docs/handover-2026-10-21.md`.

  Two things the port had to get right, because the file it writes is load-bearing and
  mostly *not* dns-sync's: a record is one line **per address** (A + public AAAA + ULA
  AAAA — the old box wrote three lines per vhost and a `*.int` name without its IPv6 is
  the RA-shaped outage in `machines/gw/README.md`), and every write is read → edit in
  memory → prove only the asked-for pairs moved → `mv -f` → reload → re-read. A failed
  read aborts rather than being treated as "empty", which would write back a file
  containing only the vhosts and take the cameras, APs, Pi Room boards and
  `authentik.int` with them.

  **network-status was the next item down this list** and is ported and moved
  (2026-10-21): it now runs on gw as a `DynamicUser`, reading `/proc`/`/sys`
  locally instead of OPNsense's commands (`docs/services.md`,
  `machines/gw/network-status.nix`).

- **Two dnsmasq bugs found by running the shadow box beside the live one**
  (2026-10-17, both fixed). Neither was visible in the config, in the build, or in
  `systemctl is-system-running`; both were visible only by asking the two routers the
  same question:

  1. **The generated records file had bare names, so the zone did not resolve.**
     `dig @10.3.1.254 services1` answered and
     `dig @10.3.1.254 services1.int.leighhack.org` answered *nothing*, while the old
     box answered the FQDN. dnsmasq's `local=` makes the box authoritative for a zone
     and does not complete a hosts entry, so every vhost, gatus endpoint and
     dns-sync name on the network would have gone NXDOMAIN at the cutover. The old
     box's `/var/etc/dnsmasq-hosts` spells both names on every line
     (`10.3.1.20 services1.int.leighhack.org services1`) and one line per address;
     `TABLES.recordsHosts` now generates that shape, and
     `machines/gw/lib/check-network-tables.nix` refuses a file whose FQDNs are
     missing or whose lines carry more than one address (dnsmasq reads the first
     field as the address and the rest as names, so an IPv4 and its IPv6 on one line
     registers the v6 address as an alias *name*).
  2. **A records change never reached the running dnsmasq.** nixpkgs' module
     restarts it only when `/etc/hosts` changes, and dnsmasq reads `addn-hosts` at
     startup, so a switch that moved a device's address wrote the file, exited 0 and
     kept answering the old address. `dns.nix` now puts the generated file in
     `restartTriggers`; verified by moving a record's address, switching, and seeing
     dnsmasq restart and answer the new value (and then moving it back).

  The habit worth keeping from this: **compare the two routers rather than reading
  the config** — `for n in <names>; do dig @10.3.1.1 $n; dig @10.3.1.254 $n; done`
  is a few seconds and finds the class of bug where the new box is self-consistent
  and wrong.

- **The six `iptables-nft` tables on the router are tailscaled's, not a second
  firewall** (checked 2026-10-17). `nft list tables` on `gw` shows `filter`/`nat`/
  `mangle` for both families alongside this flake's `router`/`routerIn`/`routerNat`/
  `router6`, and `nft` warns `table ip filter is managed by iptables-nft, do not
  touch!`. They are `ts-input`/`ts-forward`/`ts-postrouting`, inserted by
  `services.tailscale`; `ip filter FORWARD` has policy `accept` and only jumps to
  `ts-forward`, and there is no `inet firewall` table (NixOS's host firewall is not
  adding a second set of decisions). So there is nothing to clean up — but at the
  cutover, when forwarding starts working or not, know which chains are whose before
  blaming `firewall.nix`.

- **`gw` is the network's router as of 2026-10-21** (cut over from co-existence
  mode, `./coexist.nix`'s `enable = false`). The box is a Supermicro X9SCL/X9SCM that
  used to run Windows; `machines/gw/README.md` has the install and the layout, and
  `docs/gw-cutover-attempt-2026-10-21.md` has the post-cutover findings (missing
  `*.int` records, no inbound NAT, dnsmasq's RA timer stopping after a LAN link flap)
  and the last four open items below. The `10.3.1.254` co-existence address is history.

- **dnsmasq stops sending *unsolicited* RAs after a LAN link flap** (found
  2026-10-21, on the cut-over box after the trunk was re-cabled; the trap and its
  checks are in `machines/gw/README.md`, "The third way dnsmasq breaks the LAN
  silently"). Solicited RAs keep working, so a probe hides it; clients lose their IPv6
  default when the 1200 s router lifetime expires, and a host that forwards (services1)
  never takes the RA default at all and so cannot reply to VLANs 226/227. The fix is a
  `systemctl restart dnsmasq` after the flap — there is no config change that restarts
  the timer. **Open**: a link-flap hook (or moving RAs to radvd, which re-advertises on
  link-up) so this cannot be missed by whoever is not watching at the time.

- **`yunohost.leighhack.org`'s certificate expired 2026-10-21 16:59 UTC** (found the
  same evening, from the browser warning). The DNAT now sends the name to the right
  place — `10.3.1.15`, ports 80/443 open, `openssl s_client` reports
  `CN=yunohost.leighhack.org` issued by Let's Encrypt — but `notAfter` had passed, and
  the renewing HTTP-01 challenge can only reach the appliance on `:80` now that the
  cutover's inbound NAT exists. Renewal is a Yunohost-appliance action
  (`yunohost domain cert renew`, or the admin UI) and **the appliance is out-of-band**
  (the machine-hop key is not authorized on `10.3.1.15`), so it needs someone with its
  credentials. Do not "fix" it by adding a services1 vhost: Yunohost terminates its
  own TLS and generates absolute `https://` URLs.

- **`aaisp_ppp_chat_secrets` does not exist** (found while writing
  `machines/gw/sops.nix`, 2026-10-14). The tunnel's credentials have never been in
  `secrets/secrets.yaml`: the old box kept them in OPNsense's own config and nobody
  has read them off it. sops-nix fails *activation*, not evaluation, on a missing
  key, so a `gw` that declares the secret and lacks it would refuse to come up — which
  is why `machines/gw/sops.nix` now declares it only when the tunnel is enabled, i.e.
  only when co-existence is off. So this is no longer a reason the box cannot boot;
  it is still the reason the box cannot dial. Read them from the old box's
  `/conf/config.xml` and `echo … | sops edit secrets/secrets.yaml` — never into a
  transcript (rule 13).

- **Nothing obtains the network's IPv6 prefix** (`machines/gw/networking.nix`,
  2026-10-14). The RA ranges in `common/vlans.nix` and the fixed v6 addresses in
  `common/dns-records.nix` are correct and dnsmasq advertises them, but no part of
  the new router's config asks the uplink for the /48, so IPv6 to the outside world
  will not work on a cutover today. How the old box got its prefix (DHCPv6-PD from
  AAISP, or a static assignment) is the first thing to read off it. Invisible on the
  shadow box, which does not advertise at all in this mode.

- **The new firewall's rules are shape-correct, not value-verified**
  (`machines/gw/firewall.nix`, 2026-10-14). nixpkgs validates the ruleset against a
  user-mode kernel at build time — which caught two real mistakes (ICMPv6's ND types
  are `nd-neighbor-solicit`, not `neighbour-solicitation`; `packet-too-big` is ICMPv6
  only) — but "parses" is not "is what the old box did". The old box's
  `/var/etc/nftables.conf` and `nft list ruleset` have never been read; do that
  before trusting any rule here, and note the per-VLAN anti-spoof rule emits *two*
  match forms (`vlan id N` and `iifname "vlanN"`) because which one the kernel
  actually matches on is a question about the real box. The shadow box loads the
  ruleset and forwards nothing, so this is still unproven.

- **A stale headscale node called `gw` may own the name** (noticed 2026-10-17,
  during the install). `sudo headscale nodes list` on services1 listed a `gw` at
  100.64.0.11, registered 2025-08-25 and offline since 2026-04-13 — some earlier life
  of this box, or of the old router's tailnet client. Nothing in the flake references
  that address, but headscale's node names are what ACLs and humans read, so a second
  `gw` is a finding either way: rename or expire the old node at the cutover, and
  check `tailscale status --json` on the new box says it advertises no routes.

- **The LAN port is doing 100 Mbit, not gigabit** (measured on the box,
  2026-10-17: `eno1` at 100Mb/s, full duplex). Either the cable, the SFP/PHY setting,
  or the switch port it lands on. It does not matter while the box is a shadow and it
  matters enormously when it is the router, because every byte to and from the
  network passes through that one interface. Check the switch port's own config before
  blaming the NIC (`crates/switch-gui/` reads the ProCurve's port state).

- **`mitigations=off` is deliberately NOT on this box's kernel command line**
  (`machines/gw/hardware-configuration.nix`, 2026-10-17). The other machines carry it
  on the "trusted LAN, single operator" argument in `common/base.nix`; a router is the
  one box where that argument is weaker (it is the box with an interface towards the
  internet) and the decision belongs to whoever cuts it over, not to whoever installed
  the OS. Say so in the README when it is decided either way.

- **Which physical port is the LAN is an assumption**
  (`machines/gw/hardware-configuration.nix`). The two `links` rules pin `router-lan`
  and `router-uplink` to the two e1000e MACs, and the assignment came from "this one
  is carrying the network today / that one has no carrier", which is strong evidence
  and not proof. Re-check at the cutover, when the AAISP handoff cable actually moves:
  `ip -br link` and which port answers.


## Postgres

- **The application databases still record collation version 2.42** while the box
  runs 2.44, so each still logs `database "…" has a collation version mismatch` on
  connect. `template1`/`postgres` were refreshed on 2026-10-13, which is what
  unblocked `CREATE DATABASE` (the full story, including why `template0` refuses, is
  in `docs/work-log-2026-10.md`). Refreshing an application database is a one-liner
  each — `ALTER DATABASE <name> REFRESH COLLATION VERSION;` — but it is a *statement
  about their indexes*, so it wants a decision per database rather than a loop. The
  strict alternative is `REINDEX` of the collated indexes.

## Switches / RADIUS

- **Rotate the RADIUS shared secret** — it was printed into a session transcript on
  2026-10-12. Both ends: the switch (`radius-server key`) and the authentik RADIUS
  outpost. `docs/procurve-switches.md`.
- **Fix the authentik `agent` password** so RADIUS stops refusing every switch
  login; until then every GUI refresh writes two `login_failed` events and
  `login_failed` alerting is permanently noisy. `docs/procurve-switches.md`.
  Related tidy: `RadiusProvider` pk 7 has `authentication_flow = None` while the
  flow actually used is `radius-authentication-flow`.
- **`switch-gui` still needs its secret committed on services1.**
  `secrets/secrets.yaml` carries the `switch_gui_env_file` key (4 keys:
  `SWITCH_GUI_LOGIN`, `SWITCH_GUI_PASSWORD`, `SWITCH_GUI_OIDC_CLIENT_ID`,
  `SWITCH_GUI_OIDC_CLIENT_SECRET`) but the change is **written and uncommitted**,
  and services1/services2 get `git reset` to `origin/main` periodically.
- **`switch-gui`: the VLAN-management commands have never been sent** (create,
  rename, delete, the per-VLAN `tagged` endpoint).
- **`switch1.int` / `switch2.int` / `switch3.int` are not dnsmasq records**, so the
  gatus "Network Switches" pings use the addresses from `common/hosts.nix`. Add the
  names to the router's host overrides if they are wanted, or drop the idea and keep
  the IP pings.

## Print servers

- **`3d-blue` still carries the cloned SD identity** (machine-id and SSH host keys);
  lime was regenerated on 2026-10-03. Harmless while blue is off-network, but do it
  if blue is ever re-imaged. The `hackspace` password and `authorized_keys` are
  shared by the clone — rotate them if the boxes should not be interchangeable.
  `docs/printer-servers.md`.

## Authentik

- **`RadiusProvider` / `groups` ScopeMapping tidy.** The `switch-gui` provider's
  `groups` scope reuses gocardless-dashboard's ScopeMapping row rather than owning
  one, because creating a fresh child row through the ORM trips a unique-violation
  on `propertymapping.name`. Fine for now; the proper fix is the web UI or the real
  API. `docs/authentik-oidc-notes.md`.

- **The authentik cutover is not done — and the shadow is now parked.** The
  container stack was deployed, migrated from the live database and verified
  against the appliance (`docs/authentik-container-migration.md`), then switched
  off on 2026-10-15 by un-importing `machines/services1/services/authentik.nix`;
  10.3.1.36 is the IdP. **Step nought is uncommenting that import and
  `sudo authentik-migrate all --force`** (the state under `/srv/authentik` is
  still there but stale). Then the four things the checklist names: the outpost
  containers (needs the two `AUTHENTIK_TOKEN` sops secrets, then
  `enableOutposts = true`), the RADIUS clients that name 10.3.1.36 (switch NVRAM
  — ask first), `authentik.int`'s router override to 10.3.1.36 (aibox's sssd),
  and flipping `shadow = false` while deleting the `proxyPass` vhost from
  `machines/services1/http.nix` in the same change.

- **Rotate the appliance's postgres password (2026-10-15).** The first 40
  characters of `PG_PASS` and of `AUTHENTIK_SECRET_KEY` reached a transcript
  through a verification command whose redaction did not work. The database
  password is cheap: it is only read by the appliance's own compose file, so
  change `PG_PASS` in `/srv/authentik/.env`, `docker compose down && up -d`.
  `AUTHENTIK_SECRET_KEY` is not cheap — authentik cannot re-encrypt what it has
  already encrypted, so rotating it means regenerating every stored provider key
  and bind password. Both are now also in `secrets/secrets.yaml`
  (`authentik_env_file`), which is where they should have been.

- **`id-shadow.int.leighhack.org` has no DNS record.** The vhost works and was
  tested with `curl --resolve`; `sudo dns-sync sync` adds the record, and
  `just dns-sync prune` should remove it when the shadow goes away.

- **The nightly authentik dump timer is off** while `shadow = true` (a dump of a
  copy of the live database would only confuse a restore). The unit itself is
  tested; it turns on with the flag. `machines/services1/services/authentik.nix`.


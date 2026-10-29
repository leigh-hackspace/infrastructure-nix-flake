# AGENTS

Guidance for AI agents working in this repository. Read this before making changes.

**This file is rules and hard-learned lessons. Nothing else.** The detail — what each
service does, how the build is wired, per-device quirks, command reference, the
narrative of what was fixed — lives in `docs/` and the crate READMEs and is linked
from here. If you learn something, add it to the doc that owns the topic (or a new
one) and leave a one-line pointer here; **if it would only ever be read by someone
touching one crate, it does not belong in this file.** When a rule below says "read
X first", X has already cost someone a day.

## Read before you touch

| You are about to… | Read |
| --- | --- |
| change any service's behaviour, flags, API, ports, or deploy | `docs/services.md` (the crate's README too where one exists) |
| touch how Rust is built: `common/crane.nix`, a `Cargo.toml`, `build.rs`, a Dioxus SPA | `docs/building-rust-code.md` |
| run non-trivial nix, debug a wedged/hanging nix, build detached, remote-build | `docs/nix-dev-workflow.md` |
| start a session, or ssh between boxes — **which box you are on is a fact to read, every box has this repo and the hop key** | rule 27 (`hostname` first) + `docs/nix-dev-workflow.md` ("Which box am I on?") |
| touch DNS, dnsmasq, the router, the DigitalOcean zone, a new vhost's name | `docs/router-and-dns.md` (+ `machines/gw/README.md` — this box **is** the network's router, live at 10.3.1.1 since the 2026-10-21 cutover: it says what is proven on that box, what is still a guess, and what must not move) |
| move TLS termination, a vhost, the nginx-sso gate, `anubis` or `headscale` **between boxes** — i.e. work the router-as-front-door plan is preparing | `docs/router-front-door-migration.md` (**deployed 2026-10-24: `gw` terminates everything the network has *one* of — cert, public + `*.int` vhosts, `nginx-sso`, `anubis`, the ACLs — and `services1` is an *origin*: plain HTTP on :80, `CONFIG.ROUTER_ONLY`, no cert, no gate, no anubis. `headscale`/`headplane` also run on `gw` now. Phase-by-phase history and the deploy findings are in that doc**) + `docs/router-and-dns.md` |
| ask whether the router can be **redundant** — a second `gw`, or CARP / VRRP / keepalived / conntrackd, or "OPNsense did this with CARP" | `docs/router-ha.md` (**there is one router, and the retired appliance never configured CARP or pfsync either; the gate is an AAISP question about whether the routed `/29` and the delegated `/48` follow a failover dial, not a NixOS module; and nothing in this flake can tell a second `gw` from the first, because the network-tables guard reads tables and never asks which box is evaluating them**) |
| touch the ProCurve switches (device side, RADIUS, logins) | `docs/procurve-switches.md` + `crates/switch-gui/README.md` |
| touch NAS mounts, the NAS itself, or backups/borg | `docs/nas-and-borg.md` |
| touch the UniFi controller, its backups, or UniFi OS Server | `docs/unifi-os-server.md` — the appliance is a *downgrade* of the running controller, and `.unf` backups are encrypted with a key that is not on disk |
| touch a Proxmox VM on monster1 | `docs/monster-proxmox.md` (+ `docs/monster-powercut-runbook.md` if anything is stopped) |
| touch the 3D print servers | `docs/printer-servers.md` |
| touch the UPS, the power-cut behaviour, the fleet shutdown, or ask what happens when the mains go | `docs/ups-and-fleet-shutdown.md` (+ `machines/gw/ups.nix` for the deployed policy: the 300 s grace, the notice texts, the target list). **The UPS is on the router**, HID-only, so nothing sees it but NUT; the countdown is `upssched`'s, not Rust's; the outage and the shutdown both `wall` every server first, and that notice is the only intervention window there is |
| touch the OpenIPC cameras (`cam5`/`cam7`/`cam9`, 10.3.2.10/.12/.17), their feeds into frigate, or one camera's config | `cameras/README.md` (+ `cameras/c110.md` — the Tapo C110 hardware and reflash story behind them; a camera that boots but has **no WiFi** → `docs/camera-wifi-recovery.md`, which is the card-only recovery and says where the credentials actually live). Asking which firmware these should run instead? `docs/camera-thingino-evaluation-2026-10-25.md` measured it: thingino is Ingenic-only, these three are SigmaStar, and `cam1` is a third family again |
| flash or re-flash a camera's firmware, or plan to | `docs/camera-sd-card-firmware-flash.md` (measured procedure: the env's own `setsdcard`/`uknor`/`urnor`, the address map, what each command destroys, the card, the console, the recovery) + `docs/camera-firmware-audit-2026-10-25.md` (why, and what was believed before) |
| touch the netboot image the Pi Room boots, or aibox's tftp/ipxe/NFS export of it | `machines/pi-room-sys/README.md` (the image is built by aibox, not deployed) |
| touch a Dioxus SPA (any of the four) or its headless-browser suite | `docs/filestore-gotchas-2026-10-05.md` (the dioxus 0.7 + wasm traps apply to all of them) + `crates/filestore/tests/README.md`, `crates/switch-gui/tests/README.md` |
| create or debug an OIDC app / authentik anything | `docs/authentik-oidc-notes.md` — **every part of it has bitten once** |
| touch the authentik container stack on services1, or anything that still names `10.3.1.36` | `docs/authentik-container-migration.md` — **the flake stack on services1 *is* the IdP since 2026-10-28**; `10.3.1.36` is a second address on that box, the appliance VM is stopped, and the outposts answer RADIUS/LDAPS there |
| run a **full network upgrade** — one flake pass landed on every box the hop key reaches | `docs/network-upgrade-runbook.md` (the order, the gates, and what must *not* move) |
| are new to this flake, or are about to **restructure** `flake.nix`/`common/` rather than add a service | `docs/flake-simplification-audit-2026-10-22.md` — where each fact lives, what `just check` actually does on each box, the two secret models, and the five structural changes worth making (**its Status table is current, its body is the tree at `f923c6e` — read the table first**) |
| are about to **simplify** — delete, merge or move config, or judge what is fat | `docs/flake-simplification-pass-2026-10-24.md` — the 2026-10-24 re-measure of the same question: 91 % of the tracked bytes are one vendored asset tree, three `AGENTS.md` rules are 93 of its 151 rule-lines, 1,044 lines cannot be deployed; every finding is gated **do now / decide / don't**, and its §3 says what may **not** be cut |
| ask "why is it shaped like this?" | `docs/work-log-2026-10.md` and the dated post-mortems in `docs/` |

## The rules, in one place

1. **Rust for anything new.** No new Python services/daemons/CLI tools. Shell
   one-liners in `ExecStart` are fine for small glue; substantial belongs in Rust
   (`crates/status-dashboard/` is the house style).
2. **Shared code goes in `crates/common-rs/`, never a second copy.** Three of those
   crates exist because the code had already drifted between callers.
3. **`just check` before you deploy**, `just check-build` when anything is built.
4. **GOLDEN RULE: confirm in the same shell invocation as the switch/boot.** See
   [Deploying](#deploying). Non-negotiable, and it has rebooted machines out from
   under agents that deferred it.
5. **`git add` new files** — untracked files are invisible to the flake. Do not
   commit unless asked; staging is fine and often required.
6. **Alejandra is the only formatter** (`just fmt`, checked by `just check`). No
   hand-formatting, no nixfmt.
7. **Never put a real account in `nixbld`** — it is the pool of UIDs builds kill, not
   a permission grant.
8. **Never evaluate this repo through a path literal**; kill the survivor of an
   aborted nix call before issuing another nix command.
9. **`nix flake check --no-build` must stay warning-free** — a warning is a finding.
   Re-read the baseline at the current lock before calling an error pre-existing
   (see [Nix commands](#nix-commands)).
10. **Changed `ExecStart`/flags? Read back the rendered unit** from a real build.
11. **No `sleep` over 30 s in a tool call.** Poll. Bound every wait.
12. **A service that needs the NAS declares `after` *and* `requires`
    `wait-for-nas.service`** and keeps restarting forever.
13. **Never print a secret, a token, or a config dump you cannot keep.** Reference
    secret *paths*; do not read contents into a transcript.
14. **New vhosts need DNS and a front-door row** — a `*.int` name is a row in
    `common/int-dns.nix` (no command to run: the router renders that table) **and** a row
    in `common/front-door.nix`, which is the table `gw`'s nginx is rendered from; the guard
    in `machines/gw/int-dns.nix` refuses an evaluation where the two disagree, and services1
    refuses one where its own vhosts no longer match the DNS table. Public names are
    hand-added in DigitalOcean. A `*.int` row takes the LAN ACL by default and a public
    alias inherits whatever its row says — check what services1 does today before trusting
    either default (`docs/router-front-door-migration.md` has the two near-misses).
15. **Read the linked doc before creating an OIDC provider**, and keep new clients on
    the existing PKCE contract.
16. **Nothing writes switch NVRAM unless asked.** A reboot is the only undo.
17. **`--no-auth` / `--dev-user` are test hooks**, never production, and the loopback
    gate parses an `IpAddr` rather than string-matching `127.`.
18. **Do not move the wasm-bindgen pin to "fix" a mismatch** — all three copies move
    together or none.
19. **`crates/network-status/frontend/dist/` is committed.** Edit the SPA → rebuild →
    commit the dist. The four Dioxus `dist/`s are git-ignored and built by the flake.
20. **Check `git log -1` on a remote box before blaming a config change** for its
    behaviour — boxes have run newer generations than their checkout.
21. **Never evaluate to find out what a change does** — deploy it and read the
    rendered file (`just switch`, then `/etc/nginx/nginx.conf` or
    `$SYS/etc/systemd/system/<unit>.service`). A `nix eval --impure --expr` against
    `builtins.getFlake` cold-evaluates the whole flake, is slower than the switch
    that answers the same question, and its aborted survivor holds the big lock.
22. **Don't build on the router — and now it can't.** gw delegates every build to aibox and
    runs `nix.settings.max-jobs = 0`, so nothing compiles here and there is no silent local
    fallback to fall back to; the guard stays anyway because **evaluation is still paid on
    the box that answers DNS for the whole network**. So: every build-shaped recipe refuses
    on gw (`_build-host-guard`), and a routine deploy of this box is `just switch` **here**
    (or `just deploy gw` from another box) — the machine deploys itself, evaluates itself and
    compiles on aibox, like every other box (`just deploy <machine>` is the general form:
    ssh in, fetch the commit, switch and confirm there). `just check` is **not eval-only here
    and cannot be made so**:
    evaluating the machines reads a derivation (crane stubbing each SPA's source tree — the
    lock-file half closed 2026-10-25), so `nix flake check --no-build` needs one trivial source
    copy and refuses it, locally or over the hop. `just check` therefore **skips that step on gw
    and prints why**; the router's real gate is `just check-eval` — the assertion evals and
    `fmt-check`, which touch no machine. The same read is keyed to the flake source, so the
    flake step also fails on *any* box whose store has not evaluated this source yet (measured
    2026-10-25: one hash, aibox and gw alike) until a deploy or one full evaluation writes the
    stubs — which is why it read as intermittent for a year, and why "run it on aibox" was a
    warm store and not a workaround.
    After any switch here, `sudo systemctl start front-door-cert-check.service`: reading the
    *issuer* is the only check that can tell an issued certificate from minica's fallback.
    Mechanism, receipts, the split devShells and the exact allowed/refused list:
    `docs/nix-dev-workflow.md` ("Never build on the router", "Evaluation must not build") and
    `docs/router-front-door-migration.md` ("Certificate facts that are easy to get wrong").
23. **A parked module marks itself, not its import list.** The first line of a module
    commented out of an import list is `# PARKED — <date> (<commit>) — <the doc that
    owns it, or "no doc; the reason is this"> — re-verify with: <command>`, and the
    import list only names the file. Then
    `grep -rl '^# PARKED' machines/` *is* the list of what is not running — which it was
    not: the reason lived in a comment beside the import, invisible to grep, and a
    parked module is never evaluated, so its rot is silent. The four in tree (affine,
    samba, gitlab on services1; nvidia on aibox) each carry their own
    re-verify command, because "just uncomment it" is not one. (`strata` on aibox came
    out of that list on 2026-10-28; un-parking it is a nine-release engine bump and
    the checklist for one is now in `machines/aibox/STRATA.md`.)
24. **A vhost move is a guard move.** A name is a row in `common/int-dns.nix` (what it
    answers with) *and* a row in `common/front-door.nix` (what the door serves), and two
    evaluation-time guards refuse a half-move: `machines/gw/int-dns.nix` (gw's vhosts must
    equal that DNS table) and `machines/services1/int-dns.nix` (the origin's vhosts must be
    exactly the `upstream = "origin"` rows). The gatus endpoint guard reads
    `common/int-dns.nix` rather than a vhost list. Adding a name, moving one, why each guard
    is where it is, and what the deploy itself found:
    `docs/router-front-door-migration.md`.
25. **The remote-build hop is the daemon's ssh, not the operator's.** nix runs it as uid 0
    out of `HOME=/root`, so a working `ssh -i ~/.ssh/agent-hop-key 10.3.1.32` from your own
    account proves nothing about it, and a delegating box whose root never visited the
    builder dies seconds in with `Host key verification failed`. The pin is
    `common/ssh-known-hosts.nix` (DATA), handed to `programs.ssh.knownHosts` by
    `common/build-farm.nix`, which writes `/etc/ssh/ssh_known_hosts` — the one file every
    ssh, root's included, reads; `just known-hosts` regenerates it the day a box is
    reinstalled, and a hand-written root `known_hosts` on one box is what this replaces.
    Receipts: `docs/nix-dev-workflow.md`, "The hop itself was broken, separately".
26. **No source file over 1000 lines, if it can be helped.** `just check-source-size`
    (part of `just check` and `just check-eval`, so it runs on the router too) refuses a
    new one; the four already over are a named baseline, not a licence
    (`switch-gui`'s `switch.rs`/`shadow.rs`, `filestore`'s `server.rs`,
    `monitoring-dashboards.nix`). Split by *job*, not by line count, and a file that is
    already a collection of jobs is the one to split first: `main.rs` in
    `crates/network-status` was 1,308 lines of argument parsing, probing, rendering and
    serving before the connection table, and the crate is now twelve files totalling 3,962
    lines with the largest at 640 — nobody has to guess which half of `main.rs` a change
    belongs in any more. Machine output (`*/dist/*`) and prose are out; a Nix data table
    is in, and `monitoring-dashboards.nix` is on the baseline rather than exempt to prove
    it. If you add to a file that is already over, say so in the commit message instead of
    quietly making it longer.
27. **Read `hostname` before anything sshs — you may already be on that box.** A session
   runs on whichever box it was started on, and every flake machine holds this repo at the
   same path and its own copy of the hop key, so nothing about the prompt, the cwd or the
   git tree says which box ran it. Everything machine-shaped here keys to the **local**
   hostname rather than to what you meant: `just switch` is `nixosConfigurations."$(uname
   -n)"` (nixos-rebuild-ng resolves a bare `--flake .` to this box, so it can only deploy
   the one you are standing on), `_build-host-guard` asks `[ "$(hostname)" = "gw" ]`,
   `just check` skips its flake step on the router, `just deploy` switches *here* when its
   argument equals `hostname`. So the accident is a **hop to the box you are already on,
   which succeeds silently**: the key is authorized on every box including this one, so the
   ssh exits 0 from a nested shell that carried in none of the cwd, `git log -1`, dirty-tree
   state or tty the outer shell had just read, and the GOLDEN RULE's *same shell invocation*
   has quietly ended. Read `hostname` first; when it equals the target, run the job locally
   — `just deploy <machine>` already degenerates to exactly that, so it is safe typed on
   any box, and hand-written ssh chains are where the mistake lives. Mechanism, the measured
   self-hop, the hostname/table-key split (the router's hostname is `gw`, `common/hosts.nix`
   calls its address `router`), and how to re-verify it the next time nixos-rebuild-ng moves:
   `docs/nix-dev-workflow.md` ("Which box am I on?").
28. **A root-owned file the dashboard has to read is a module of its own, and its header is
   the decision.** `gw` has two: `/proc/net/nf_conntrack` (`0440`, group read granted by
   `machines/gw/conntrack-reader.nix`) and `/var/lib/dnsmasq/dnsmasq.leases` (`0640`, by
   `machines/gw/lease-reader.nix`). Neither permission is a line in the service's unit,
   because handing a service a root file is a question about *who may see this* — the
   conntrack table is who talked to whom, the lease file is what each device calls itself
   — and the two modules carry that answer, the measurements behind it, and the reason the
   mechanism is what it is (group + `ExecStartPre=+` for the sysctl-backed one, group +
   a `systemd.path` watch for the file dnsmasq rewrites). A group here has exactly one
   member on purpose: the dashboard's own `network-status` account. Read the module before
   adding a member or a second file to it, and make the decision explicitly rather than
   inheriting one by adding a line to a group.

---

## Layout

- `flake.nix` — the flake. **Every input is a network-wide source**: nixpkgs,
  crane, sops-nix, nixos-utils, llama-cpp, strata, nix-software-center, the door
  entry system. Nothing is read out of `~/Projects` any more — the two things
  that used to be (`whisper-ws`, `pi-room-sys`) are in-tree now, so a clone of
  this repo evaluates every machine and `just check` runs anywhere. Machines:
  `services1` (main services box, 10.3.1.20), `services2` (strata + its own status
  dashboard, 10.3.1.21 — `machines/services2/README.md`), `aibox` (10.3.1.32,
  the network's build machine since 2026-10-17), `gw` (the network's **router**, live at
  10.3.1.1 — `machines/gw/README.md`) and `pi-room-sys`
  (a machine that is *built by* aibox rather than deployed —
  `machines/pi-room-sys/README.md`).
- `docs/` — reference docs (`services.md`, `building-rust-code.md`,
  `nix-dev-workflow.md`, `router-and-dns.md`, `router-ha.md` (the router-redundancy
  decision record: what CARP-style HA maps to on Linux, and the four things it does not fix),
  `procurve-switches.md`,
  `nas-and-borg.md`, `monster-proxmox.md`, `printer-servers.md`, `follow-ups.md`,
  `work-log-2026-10.md`) and the dated narrative write-ups, post-mortems and audits
  (gtx1060 saga, filestore gotchas, the repo audit, the gw cutover attempt). They live
  here rather than next to the modules so the flake source stays config only; module
  comments point into them when they need the backstory.
- `common/` — shared modules imported by more than one machine: `tools.nix`, `users.nix`,
  `sops.nix`, `nixbld.nix` (the build-user invariant), `crane.nix` (the shared crane
  helpers), `frontend-build-spa.sh` (the one SPA build recipe all four Dioxus
  frontends call). **Every file in `common/` starts by naming its kind** — `MODULE`,
  `DATA`, `HELPER`, `CHECK`, `PACKAGE` — because the directory mixes all five and you
  cannot otherwise tell a NixOS module from a bare table from a `{lib}: …` functor **Network-wide facts live here, not per machine:** `config.nix`
  (the `LOCAL_NETWORK` ACL, the `pg_hba` table, the SSO domains, and the runtime secret
  paths — it used to be `machines/services1/config.nix`, now deleted, and its being
  per-machine is what stopped a second box serving any vhost), `hosts.nix` (one table of
  machine addresses, read by the nginx proxies, the Prometheus scrape targets and the
  gatus checks so they cannot disagree), `vlans.nix` (the VLANs, their blocks and
  their dynamic ranges), `dns-records.nix` (which device keeps which address) and
  `int-dns.nix` (which `*.int.leighhack.org` names answer with the front door's
  addresses, which answer with their own, and which the front door serves but must not
  answer) — the tables the router generates its dnsmasq conf and both hosts files and
  nft sets from, so a lease and a vhost name are each one fact in one place; they are
  network-wide for the same reason `hosts.nix` is,
  and `machines/gw/lib/check-network-tables.nix` is what refuses to let them drift,
  `nginx/`
  (`front-door.nix` — the renderer for `common/front-door.nix`; `origin-vhost.nix` —
  what `services1`'s app-shaped vhosts use now; `int-vhost.nix` — the door's TLS vhost
  builder; `int-vhost-names.nix`),
  `exporters.nix`, `build-farm.nix` (GC + remote building), `monitoring-endpoints.nix`
  (the gatus endpoint data). `common/nginx/` and `common/config.nix` are shared because
  `gw` is the TLS terminator and front door for the whole network: every name is one row
  in `common/front-door.nix`, and an origin vhost can only proxy to a service running on
  another box.
- `crates/` — **every Rust crate lives under here**, and `crates/Cargo.toml` makes the
  native ones **one cargo workspace**: one `Cargo.lock`, one `target/`, and
  `cargo build|test|clippy --workspace` (or `-p <name>`) from `crates/` covers the
  whole fleet, which is what rust-analyzer wants too. Programs are top-level
  directories; shared libraries are `crates/common-rs/` (see `docs/services.md` for
  what each owns). Five directories are **not** members, each opted out by its own
  `[workspace]` table: the four Dioxus SPAs (each `frontend/` is its own workspace
  with a git-ignored symlink for its shared path deps) and `crates/whisper-ws/`
  (a native crate whose `src/assets.rs` is generated by an npm build, so it could
  never compile as a member and is packaged by `crates/whisper-ws/package.nix`
  instead of by crane). The reasons are at the top of `crates/Cargo.toml` and the
  mechanics are in `docs/building-rust-code.md`.
- `machines/services1/` — the services box: `hardware-configuration.nix` (the NAS export
  list the mounts and automounts are generated from), `containers.nix` (podman + the
  "never give up" restart policy for all `podman-*` services), `services/` (one file per
  service), `lib/` (nginx SSO helpers + the config-to-gitlab assertions), `sops.nix`
  (which runtime secrets exist as files under `/run/secrets` — **reference the paths, do
  not read the contents**), `machines/services1/secrets/.env.sample` (the shape of the
  shared env-file secret — `secrets/` at the top level holds only `secrets.yaml`).
  `machines/services1/README.md` keeps what is not in the flake (tailscale
  client, secrets habits).
- `machines/gw/` — the network's **router**, a Supermicro X9SCL/X9SCM live at
  **10.3.1.1**: cut over from the OPNsense appliance on 2026-10-21, which is retired.
  **There is no shadow box and no second router** — `machines/gw/coexist.nix` and every
  branch of co-existence mode were deleted on 2026-10-22, so a DHCP or RA change made
  here is made to the box that hands out every address in the building. DNS/DHCP are
  generated from `common/vlans.nix` + `common/dns-records.nix` + `common/int-dns.nix`,
  nftables from `common/config.nix`, and the AAISP tunnel and tailnet client are in
  `machines/gw/`. `machines/gw/README.md` is mandatory before any change here — it
  separates what was read off the old box from what is still a guess, names the four
  checks that prove the router is doing its job, and lists what is open. Its checkout is a
  normal checkout at the deployed commit — it fetches the commit itself (`just deploy gw`,
  or `just switch` on the box), so it is no longer an rsync mirror of somebody else's working
  tree. `machines/gw/lib/check-network-tables.nix`
  runs from `just check` and as a module assertion, and it is what refuses a VLAN or
  address the two tables disagree about.
- `machines/pi-room-sys/` — the image the Pi Room computers **netboot**: a real
  `nixosConfiguration`, but one that runs on no box. aibox builds it and serves
  its kernel, initrd and squashfs store over iPXE + NFS
  (`machines/aibox/netboot.nix`), so changing it means switching aibox and
  rebooting the clients (`just switch-netboot`). `machines/pi-room-sys/README.md`
  first: it has the boot flow, the initrd-SSH exposure, and the private host key
  that came with the tree.
- `secrets/` — sops-encrypted.

## Language preference

- **Use Rust for new programs/tools.** Do not write new services, daemons,
  scripts-as-programs, or CLI tools in Python (or other languages) without a strong
  reason. See `crates/status-dashboard/` for the house style.
- Shell one-liners inside NixOS `ExecStart`/activation scripts are fine for small
  glue, but anything substantial belongs in Rust.
- New shared code goes in `crates/common-rs/` — `service` and `json`'s emitters exist
  because both had already drifted between three callers, and the 2026-10-13
  consolidation found a security bug while deduplicating them
  (`docs/work-log-2026-10.md`).

## Deploying

- Deploy with the justfile: `just switch` / `just boot` (`sudo nixos-rebuild
  switch --flake .` + `sudo nixos-confirm`, chained).
- **Check before you deploy:** `just check` — `nix flake check --no-build`, the four
  assertion evals (`machines/services1/lib/check-config-to-gitlab.nix`,
  `common/check-printer-states.nix`, `machines/gw/lib/check-network-tables.nix` and
  `common/check-doc-pointers.nix`, the last one checking the recipe names, repo paths and doc
  links these instructions promise), and `nix fmt -- --check` over every tracked `.nix`. The
  gatus endpoint guard is a module assertion in `machines/services1/services/monitoring.nix`,
  so it is the *flake* step that catches that drift — and that step is not eval-only (rule 22:
  an eval-time read of a derivation makes it need one trivial source copy), which is why on gw
  `just check` runs the evals, skips the flake step out loud, and why `just check-eval` is the
  router's gate. The build-shaped recipes (`just check-build`, `just test`, `just clippy`,
  `just build-frontend`, `just filestore-*`, `just switch-gui-test`, `just build-remote`) are
  not safe anywhere near the router and refuse on gw via `_build-host-guard`. Then, when
  anything is built rather than just evaluated, `just check-build` (`nix build` of all four
  deployable machines' system toplevels — compiles everything, activates nothing, writes no boot
  entry; it is **not** `nixos-rebuild dry-build`, which builds nothing). Budget ~75 min for
  a nixpkgs bump. `just test` runs the cargo
  tests, `just clippy` the lints, `just filestore-test` and `just switch-gui-test` the
  headless-browser suites (the latter drives a fake ProCurve console —
  `crates/switch-gui/tests/README.md`), `just build-remote` offloads a build to
  aibox.
- **GOLDEN RULE — confirm immediately after every switch/boot.**
  `system.autoRollback.enable = true` is set on **all four** deployable machines (the
  fifth configuration, `pi-room-sys`, is a netboot image with no profile to mark good).
  Read off the
  unit script rather than guessed: activating a configuration **restarts the
  auto-rollback timer**, which fires **once, five minutes later**, compares
  `/run/current-system` with `/nix/var/nix/profiles/system-good` and, if they differ,
  walls, sleeps 30 s and **reboots the box into `system-good`**. So the window is five
  minutes — but you still confirm in the same shell invocation, for three reasons: a
  late confirm marks the *old* generation good, a rollback reboots the machine out
  from under whatever the next tool call is doing, and a confirm of a **failed**
  activation marks a half-activated system good and disarms the only safety net
  (`nixos-rebuild switch` flips `/run/current-system` before it finishes, so
  activation can die at the end and still leave the new generation current — hit on
  aibox, 2026-10-15).

  ```bash
  readlink /run/current-system /nix/var/nix/profiles/system-good   # equal = confirmed
  ```

  **An activation that runs longer than five minutes outruns its own safety net.** The timer
  is armed when a configuration is activated, so a switch that takes over five minutes — one
  where `security.acme` has to issue a certificate, because `nginx` is ordered after the acme
  unit and the issuance waits out DNS propagation — has the timer fire *during* a later
  activation. It then sees `/run/current-system` and `system-good` differing (what every
  in-flight activation looks like from there), tries to act, and loses the store lock to the
  switch it is supervising: `auto-rollback.service` exits 11, `switch-to-configuration`
  reports `the following units failed: auto-rollback.service`, and `nixos-rebuild` exits 4
  **with nothing wrong and no reboot**. Read the failing unit's name before reacting; then
  `readlink` the two paths, `sudo nixos-confirm` if the running generation is the one you
  meant, and `systemctl restart auto-rollback.timer` (it does not re-arm itself). Hit on gw,
  2026-10-24, with the receipt in `docs/router-front-door-migration.md`.

  **The net is armed by an activation, for five minutes, and then it is gone by design.**
  `auto-rollback.timer` is `OnActiveSec=5min` with `RemainAfterElapse=false`: activating a
  configuration (re)starts it, it fires once, compares the two paths, and stops. So
  `systemctl is-active auto-rollback.timer` reads `inactive` on every healthy box more than
  five minutes past a switch — measured on gw and on the untouched services1, both
  `inactive` — and it is therefore **not** a health check. What is a health check is the
  comparison itself: `readlink -f /run/current-system /nix/var/nix/profiles/system-good`,
  equal means the box has decided to stay. And that five-minute window is a **countdown to a
  reboot**, which matters most on `gw`: if a switch *fails* while the new generation is
  already current, the timer is armed and the machine will reboot into `system-good` on a
  schedule. Decide inside the window — fix the activation and confirm, or let it roll back —
  rather than discovering the timer afterwards; stopping it to keep working is legitimate
  only if you finish one of those two things before it would have fired, and it re-arms
  itself at the next activation either way. (Hit on gw, 2026-10-24: a health-check unit that
  failed during activation failed the switch, which armed the countdown on the network's
  router. `docs/router-front-door-migration.md` has the outcome and the fix.)

  1. Run `sudo nixos-confirm` **immediately** after `switch`/`boot`, in the **same
     shell invocation** (`... switch ... && sudo nixos-confirm`), on the machine that
     was switched. `&&`, never `;` — and in a script, confirm only when the switch's
     own exit status is 0.
  2. Then verify `readlink -f /run/current-system` still points at the new generation.
  3. Over ssh, chain it all in one command — and `cd` first, because `--flake .`
     resolves against the cwd:
     `ssh ... 'cd ~/Projects/infrastructure-nix-flake && sudo nixos-rebuild switch --flake .#<machine> && sudo nixos-confirm'`
- **New vhosts need DNS records** before they resolve: a `*.int.leighhack.org` name is a
  row in `common/int-dns.nix` — the router renders that table into dnsmasq's `addn-hosts`
  in the same activation, and `machines/services1/int-dns.nix` refuses to evaluate
  services1 unless the table equals the `*.int` vhosts it serves. **There is no
  `just dns-sync-*` any more** (`crates/dns-sync` was deleted on 2026-10-22, along with
  the recipes); public `*.leighhack.org` names are still **added by hand** in
  DigitalOcean. The wildcard ACME cert covers both, so no cert work.
  `docs/router-and-dns.md`.
- **Long jobs are polled, never slept through** — see below.
- Formatting: the whole tree is alejandra-clean and `just fmt` keeps it that way
  (`just fmt-check`, run by `just check`, fails otherwise). Alejandra is in the
  devshell and configured in both `.zed/settings.json` and `.vscode/settings.json`, so
  editor and CI agree.

## Long jobs: poll, don't sleep

**No `sleep` longer than 30 seconds in a tool call.** A long sleep is a blind wait: it
reports nothing until it ends, so a job that failed in its first second still costs the
whole wait. There is no point sitting through five minutes for a build that has already
failed — the error is only useful if it is read as soon as it appears.

- Poll instead. Read the state, act on what it says, re-read on the next tool call. The
  loop is the agent, not `sleep` in the shell.
- Cheap polls, one per tool call: `systemctl is-active <unit>`,
  `systemctl show -p Result --value -p ActiveState <unit>`, `journalctl -u <unit> -n 50`,
  `pgrep -f <pattern>`, `tail -1 /tmp/deploy.log`, `pvesm status | grep nas2-nfs`,
  `curl -fsS -m 5 <url>`, `nixos-rebuild dry-run --flake .#<machine>`.
- Bound every wait so a hang reports as a failure rather than as a tool-call timeout:
  `curl -m 5`, `ssh -o ConnectTimeout=5`, `timeout 60 ...`. A poll that times out *is*
  the answer.
- Short sleeps are fine when they are the only way to catch a transition (a timer that
  fires in a few seconds, a service settling after a restart, a retry backoff). The rule
  is about waiting for a *result* that could have been read immediately.
- Anything that genuinely takes minutes goes in a detached unit writing to a log (the
  `systemd-run` recipe is in `docs/nix-dev-workflow.md`), then gets polled: check whether
  the process is still running, read the log, stop the moment it exits — not at the
  estimated duration. Polling also catches the failure modes a sleep hides: a build that
  dies immediately, a unit that exits 0 but did nothing, a remote builder that silently
  falls back to local.
- **An aborted tool call does not stop what it started, and a nix client holds the global
  store lock for its whole evaluation.** A `nix eval` whose tool call reported
  `Command aborted` once went on running for minutes holding
  `/nix/var/nix/db/big-lock`; every nix command after it queued behind the lock and did
  *nothing* — no CPU, no output, no timeout — which is indistinguishable from "this eval
  is just slow". **After aborting a nix tool call, find the survivor and kill it before
  issuing another nix command**; when a nix command hangs, look for the lock holder
  before believing it is evaluating: `pgrep -a -f "nix eval|nix build"`. Full triage:
  `docs/nix-dev-workflow.md`.

## Nix build users: `nixbld` is a target, not a grant

**Resolved 2026-10-12.** The 2026-10-11 incident — every nix build on services1 and
services2 SIGKILLing the admin's entire process set — was caused by this repo putting
`leigh-admin` in the `nixbld` group. `build-users-group` is not a permissions group, it
is the **pool of UIDs nix runs builds as**: nix takes a member of it for each build and,
both before and after that build, runs `setuid(uid); kill(-1, SIGKILL)` (`killUser()`) to
sweep the build's leftovers. A real account in that group therefore loses every process
it owns to every build on the machine — which is why the sweep looked indifferent to who
asked, to whether the build succeeded, and to which box it ran on. Full story, nix
2.34.7 source references and evidence: **`docs/nix-build-sigkill-2026-10-11.md`**. The
invariant, its assertion and its self-healing timer live in **`common/nixbld.nix`**.

- **Never put a real account in `nixbld`.** Not for a build farm, not "so the daemon
  accepts this client": that is `nix.settings.allowed-users` (already `*`) plus
  `nix.settings.trusted-users`, and `/nix/var/nix` is root-only so a non-root nix reaches
  the daemon socket either way. `common/nixbld.nix` **fails the eval** if the config asks
  for it, and its `nixbld-guard.timer` undoes an imperative `usermod -aG nixbld` /
  `gpasswd -a` within fifteen minutes and writes the reason to the journal. If a session
  ever dies during a build again, `journalctl -u nixbld-guard` is the first thing to
  read.
- **Ordinary deploys are safe**: `just switch` / `just boot` as `leigh-admin`, with the
  GOLDEN RULE confirm chained. The `deploy` account is dead weight and a workaround for
  a fixed bug — do not reach for it (`docs/follow-ups.md`).
- **Interrupting a build is harmless.** The sweep now lands on a throwaway `nixbldN` uid
  that owns nothing.
- **Remote building is on and is the default path.** With `nix.distributedBuilds = true`,
  nix 2.34 *prefers* the external builder and only falls back to local when no builder is
  available — `--max-jobs 0` does not enable delegation, it removes the local fallback.
  So a plain `nix build` / `just switch` on services1 builds on aibox and copies the
  result back. Flip `nix.distributedBuilds = false` in `common/build-farm.nix` to go back
  to local-only. Two settings beside it were unexamined until 2026-10-25 and both matter:
  `builders-use-substitutes = true` (off by default in nix, so until then the *delegating*
  box substituted every input and uploaded the closure over the hop with aibox's job slots
  idle — the shape of "the build farm is slow"), and `sandbox = "relaxed"` on **services1
  only**, for the one package whose build wants the network; the one-line recipe that says
  whether a box needs it is in `machines/services1/configuration.nix`, and `sandbox` is not
  a sops setting (`extra-sandbox-paths` is). Detail: `docs/nix-dev-workflow.md`,
  "Remote building".
- `network-status.service` no longer runs as `User=leigh-admin`: it moved to gw on
  2026-10-21 and runs as its own `network-status` system account reading local `/proc`
  (`machines/gw/network-status.nix`) — not a `DynamicUser`, because the per-connection
  table it serves is `/proc/net/nf_conntrack` and that is group-readable to one group
  (`machines/gw/conntrack-reader.nix`, whose header has the measurements).
  The follow-up that tracked it is closed in `docs/follow-ups.md`.

## Nix commands

The full command reference, the rendered-unit workflow and the gotchas are
**`docs/nix-dev-workflow.md`**. The five that bite:

- **Never evaluate this repo through a path literal.** `import
  /home/leigh-admin/Projects/infrastructure-nix-flake/common/crane.nix` resolves the
  `../crates` inside it against the **real working tree**, and `lib.cleanSourceFilter`
  does **not** read `.gitignore`, so it copies the cargo and dioxus `target/` trees
  (7.2 GB, plus 516 MB of `.git`) into the store while holding the big lock.
  Through `self` the same directory arrives git-filtered, so
  nothing in a deploy hints at this. Read values out of the flake instead
  (`nix flake check --no-build`, `nixos-rebuild dry-run`, then
  `$SYS/etc/systemd/system/<unit>.service`). **`builtins.getFlake "/abs/path"` is the
  same trap and looks like a flake reference**: it is a `path:` ref, so nix copies the
  whole working tree (`.gitignore` is not read) — **7.7 GB against the 26 MB of
  tracked source, 2–6 min, re-copied on every call, even when nothing changed at all**
  (an untouched repeat re-streamed all 7.7 GB in 303 s and threw it away; one byte
  inside `crates/target/` costs a fresh 7.7 GB store path). `nix eval
  ".#…"` uses the 26 MB git-filtered source and answers in a second — dirty tree
  included, the git filter ignores the build trees entirely; to deploy a dirty
  tree the same way, `--flake "git+file://$PWD?dirty=1#<machine>"` (6 s on services1).
  Numbers, the fat `-source` cleanup and the `nix store delete` gotcha: `docs/nix-dev-workflow.md`.
- **Evaluation must not build anything.** If `nix flake check --no-build` reports
  `path '…-src.drv' is not valid`, or an eval prints `building '…'`, something reads a
  **derivation** at eval time (`builtins.readFile`/`pathExists` on a store path
  realises it, and `--no-build` does not stop that — it only declines to build the
  command's own goal). This repo does it on purpose in `common/crane.nix` (a
  content-keyed `Cargo.lock`, stubbed member sources), so crane must be handed
  `cargoVendorDir`/`cargoLockContents` and never left to read `src` or the lock
  itself; `wasmSpa` takes `cargoLockPath` for that reason. Measured cost before the
  fix: aibox's config evaluated in 56 s because it built on aibox. Mechanism, the
  A/B that proves it, and why a *passing* `--no-build` is not the proof: `docs/nix-dev-workflow.md`.
- **Never evaluate to find out what a change does.** `nix eval --impure --expr` with
  `builtins.getFlake` starts a **cold** evaluation of every machine and crate, so it
  reuses nothing the last deploy cached and is *slower than `just switch`* for a
  one-service change — and an aborted one keeps running while holding the big lock,
  which makes the next `just switch` look hung. Apply it and read the rendered file;
  to check without applying use `nixos-rebuild dry-run`/`dry-build`, which is the same
  evaluation the deploy does and so stays cache-warm — **neither of them builds
  anything**; to prove a change compiles, `just check-build`. Full ordering of
  methods: `docs/nix-dev-workflow.md`.
- **`nix eval`/`nix build` on a bare path** from the repo root resolves against the *repo*
  flake, not nixpkgs.
- **`nix flake check --no-build` is expected to be warning-free**, so a warning is a
  finding. The two at the 2026-10-14 lock (Grafana's plaintext `secret_key`, and four
  `stdenv.is*` deprecations from the CUDA package set behind `packages.strata-cuda`)
  are open in `docs/follow-ups.md`. The `error: path
  '...sops-secret-grafana_secret_key.drv' is not valid` that used to end this command
  **did not reproduce at the 2026-10-14 lock** — a baseline is a fact about a lock,
  so re-read it rather than inheriting it, and **verify against clean HEAD (`git
  stash`) before hunting any error you think your change caused.**
- **Editing a service's `ExecStart`/flags: always re-run `nixos-rebuild build` and read
  back the rendered unit** — NixOS normalises quoting and expands `${...}` and
  line-continuations differently from what you typed.

## Infrastructure facts

The addresses are `common/hosts.nix` (services1 10.3.1.20, services2 10.3.1.21, aibox
10.3.1.32, router 10.3.1.1, nas 10.3.1.6, switch1/2/3 10.3.1.2/.3/.4). Everything not
listed here is in the doc named for it.

- **NAS** is TrueNAS at 10.3.1.6; NFS shares `/mnt/cameras`, `/mnt/filestore`,
  `/mnt/backups`; slow to come up after a power cut; `leigh-admin` has passwordless sudo
  **as the middleware user record**, never by editing `/etc/sudoers` (middleware
  regenerates it and the generated file has no `#includedir`). → `docs/nas-and-borg.md`.
- **NAS dependency rule:** any service that needs the NAS declares
  `after = [ "wait-for-nas.service" ]` **and** `requires = [ "wait-for-nas.service" ]`
  (the unit is generated by `common/nas.nix`, which also generates the mounts and their
  automount units from the per-machine export list), and must keep restarting forever —
  the container policy in `containers.nix` (`Restart=always`, `StartLimitIntervalSec=0`)
  applies automatically to all `virtualisation.oci-containers.containers`.
- **Automount gotcha:** `x-systemd.automount` in mount `Options=` is only honoured by
  systemd-fstab-generator. For unit-file mounts (what NixOS `systemd.mounts` generates)
  it is dead config — the `.automount` units must be defined explicitly, which
  `common/nas.nix` does from the same list as the mounts so the two halves cannot drift.
  Never rely on the mount option alone.
- `wait-for-network.service` (`common/tools.nix`) pings the NAS; both it and
  `wait-for-nas.service` have `TimeoutStartSec = 0` and **must stay that way** — "never
  give up" is a hard requirement of this infra.
- **NixOS systemd units:** `Restart`/`RestartSec` live in `serviceConfig`;
  `StartLimitIntervalSec`/`StartLimitBurst` live on the unit itself
  (`systemd.services.<name>.startLimitIntervalSec`), **not** in `serviceConfig`.
- **nginx:** `gw` is the front door. Every name is a row in `common/front-door.nix`,
  rendered by `common/nginx/front-door.nix`; `*.int` names also live in
  `common/int-dns.nix`. The ACME cert (`security.acme` on `gw` only) covers
  `leighhack.org`, `*.leighhack.org`, `*.ai.leighhack.org` and `*.int.leighhack.org`
  (DNS challenge), and `front-door-cert-check.service` is what proves it is CA-issued
  rather than the minica fallback. `nginx-sso` and `anubis` run on `gw`, both on
  loopback; public rows with `sso = true` inject `X-WEBAUTH-USER`, LAN rows use
  `CONFIG.LOCAL_NETWORK`, and an origin uses `CONFIG.ROUTER_ONLY` on plain HTTP :80.
  `services1` has no certificate, no gate and no anubis — putting one back is how a
  second front door gets created.
- **Core switches** are the ProCurve 2510G-48s at 10.3.1.2 (`switch1`) and 10.3.1.3
  (`switch2`) plus the 2824 at 10.3.1.4 (`switch3`), managed over **Telnet only** (no
  API). Their `agent` login goes out over **RADIUS (authentik)** and falls back to the
  switch's local account database when RADIUS refuses, which is what makes it print
  `Can't reach RADIUS server.` **on a successful login** — it is this firmware's generic
  "authentication failed" message, and it is noise, not an incident
  (`docs/procurve-switches.md`). `agent` is a **Manager**: `enable` is a second login,
  writes run inside config mode, and **nothing writes NVRAM**, so a reboot undoes any
  change. `crates/switch-gui/` wraps them; its README is the CLI's trap list.
- **Router** is `machines/gw/` — the flake's own NixOS box at **10.3.1.1**, cut over
  from the OPNsense appliance on 2026-10-21 (the appliance is retired). DNS/DHCP come
  from `common/vlans.nix` + `common/dns-records.nix`, nftables from `common/config.nix`,
  and the AAISP tunnel and tailnet client are in `machines/gw/`. **Egress is split by
  VLAN:** VLAN 225's IPv4 leaves through the AAISP tunnel, every other VLAN's IPv4 goes
  straight out of the hub with a second NAT, and **all IPv6 on every VLAN leaves through
  the tunnel** — so a dead tunnel is a network with no IPv6 at all, and VLAN 225 must stay
  on the tunnel in both directions because its inbound arrives there
  (`docs/router-and-dns.md`, "Which uplink each VLAN leaves by"). Read
  `machines/gw/README.md` before changing it and `docs/gw-cutover-attempt-2026-10-21.md`
  for what broke at the cutover. **Never edit the generated
  `/etc/dnsmasq.d/network.conf`.** The flake-owned records are
  `common/dns-records.nix` (devices) and `common/int-dns.nix` (the `*.int` vhost names,
  rendered into `/etc/dnsmasq-int-vhosts.hosts`); both are store paths and both are
  dnsmasq `restartTriggers`, so a record and the restart that loads it land in the same
  activation. `crates/dns-sync` — which used to ssh from services1 and append lines to a
  mutable `/var/lib/dnsmasq/names.hosts` — was deleted on 2026-10-22, together with the
  `just dns-sync-*` recipes (`network-status` had already moved onto the box on
  2026-10-21 and runs as a DynamicUser reading local `/proc`).
  **dnsmasq's unsolicited RAs stop after a `router-lan` link flap: restart dnsmasq after
  any cabling** or WiFi clients lose their IPv6 default 20 minutes later. SSH as
  `leigh-admin@10.3.1.1` (root is not keyed); DO token is `DO_AUTH_TOKEN` in the shared
  env-file secret (`CONFIG.ENV_FILE`) — **never print it or copy it off services1.**
  → `docs/router-and-dns.md`, and the open items are in `docs/follow-ups.md`.
- **OpenIPC cameras** `cam5`/`cam7`/`cam9` (10.3.2.10/.12/.17) and `cam1` (10.3.2.6) are Buildroot boxes on VLAN 226: **no NixOS, no systemd, no flake module** — their whole state is `/etc` on a **704 KB jffs2 volume on the flash's own settings partition** (`/dev/mtdblock4 on /overlay type jffs2`; `/` is an `rw` overlay on top of it, 460 KB free), so **a write to a camera persists and a reboot undoes nothing** — this file said "tmpfs, re-read by init on boot" until 2026-10-25 and was wrong — and a kernel/rootfs reflash leaves that partition alone. A camera is managed by whole-file replace + md5 verify + `/etc/init.d/S95majestic restart`, never by a deploy, and `autoRollback`/GOLDEN RULE mean nothing there. services1's frigate pulls their RTSP through its own go2rtc, whose feed-name→camera table is **inside the container** (`/dev/shm/go2rtc.yaml`, generated from `/srv/frigate/config/config.yaml`, a runtime bind mount the flake only mounts), so both ends of the pipe are off-flake. The verbs are `just cam-probe/cam-check/cam-pull/cam-diff/cam-push/cam-init/cam-key` and, for firmware, `cam-preflight`/`cam-flashdump`/`cam-backup`/`cam-image`/`cam-env`/`cam-card` — **there is no update recipe, and only `cam8` has been flashed** — a rootfs rollback, then the `nightly-20261010-a40b399` update on 2026-10-28, **rootfs *and* kernel**: that build's WiFi driver oopses on the 2024 kernel, and it prunes `sc3338` while cam8 is genuinely `sc3338` (`docs/camera-sd-card-firmware-flash.md` §9a): the layout is **fixed-slot** (`256k(boot),64k(env),2048k(kernel),5120k(rootfs),-(rootfs_data)`, the whole 8 MiB, identical on all three), so a flash **does not move the settings** — what it threatens is `sf write`, which writes `${filesize}` bytes and never consults `rootsize`, so an oversized rootfs lands on the settings partition and **the only size check in the system is the one in the guide** (the tools' own checks fail open, and `-f` disables them). The flashing mechanism **needs no U-Boot work**: every env ships `setsdcard` (`fatload mmc 0`) + `uknor`/`urnor`/`ubnor` — but **`urnor` is unusable as shipped** (measured: the erase is refused, the write goes ahead, `Written: OK` lies; use `sf update` + a read-back `cmp.b`, `docs/camera-sd-card-firmware-flash.md` §8). Do not `fw_setenv`: `bootcmd` lives in the one partition a kernel/rootfs flash cannot restore, and `ubnor` erases boot **and** env. OpenIPC's `failsafe` rescue OS is gk7205-only, so a crash-loop here has no rescue — but a dump plus Linux booting means `flashcp` restores any partition with no console at all. **`docs/camera-sd-card-firmware-flash.md` is the procedure; `docs/camera-firmware-audit-2026-10-25.md` is the reasoning and the strike list;** `cameras/inventory.sh` is busybox-ash POSIX only, and `/var/log` is a symlink to `/tmp` so **a reboot wipes the log ring that recorded why it rebooted** (majestic's `watchdog` reboots a hung one — an unexplained boot on a camera is a sensor fault). Table, proven-vs-guess, and the faults found 2026-10-25 → `cameras/README.md`.
- **3D print servers** `3d-blue` (10.3.14.62) / `3d-lime` (10.3.14.61), Klipper +
  Moonraker, user `hackspace`, polled by `moonraker-exporter` at **fixed IPv4 on purpose**
  (the two Pis were cloned from one SD card, so their DNS names used to advertise each
  other's addresses). → `docs/printer-servers.md`.

## Authentik (id.leighhack.org — services1, and 10.3.1.36)

**The IdP is the podman stack on services1** (`machines/services1/services/authentik.nix`),
cut over on 2026-10-28 from the docker-compose appliance that used to own **10.3.1.36**.
The address came here rather than the clients being reconfigured, because two groups of
clients cannot be reconfigured: the ProCurve switches' `radius-server host 10.3.1.36` in
NVRAM (rule 16) and every LDAP client dialling `authentik.int.leighhack.org`, which
`common/dns-records.nix` answers with `.36`. So `10.3.1.36` is a **second address on
`vlan225`** (`machines/services1/networking.nix`, after `.20` so `.20` stays the source of
everything else), the outpost ports are published on `0.0.0.0` on purpose so conntrack
answers as the dialled address, and `id.int.leighhack.org` is an `origin` row in
`common/front-door.nix`. `docs/authentik-container-migration.md` owns the procedure, the
receipts and the four things that are easy to get wrong.

- **`staged` in that module is the gate**: `true` runs the stack without claiming the
  network (no vhost, no outposts, no worker, no dump timer) — how the migrated copy was
  verified while the appliance still answered. Flipping it is a deploy-time decision, not
  a runtime one, and it does **not** withdraw `.36`; stopping the old serving side and
  clearing the flag belong in the same change, because two containers registering one
  outpost token against two authentiks is how the switch logins break.
- **`AUTHENTIK_SECRET_KEY` is load-bearing and unrotatable.** It is in
  `secrets/secrets.yaml` as part of `authentik_env_file` because the copied database
  cannot be opened without it, and authentik cannot re-encrypt what it has already sealed:
  never rotate it. Same for the outpost tokens
  (`authentik_radius_outpost_env`, `authentik_ldap_outpost_env`), which live in the
  database and migrate with it.
- **Upgrades happen here now** — bump the three image pins in the module, same-major
  postgres, and the appliance's `check` parity is history (it existed because a database
  restored into a *different* authentik version is migrated on first boot and cannot be
  un-migrated; that was the rollback path and it closed with the VM).
- **An origin rewrites `X-Forwarded-Proto` and `X-Real-IP` unless told not to.** nixpkgs'
  `recommendedProxySettings`, which `common/nginx/origin-vhost.nix` puts in every origin, sets
  `$scheme` and `$remote_addr` — so behind the door an app sees `http` and `10.3.1.1` for
  every client, and anything building absolute URLs from the request emits `http://` into an
  HTTPS page (authentik's mixed content, 2026-10-28). `preserveForwardedHeaders = true` opts
  a vhost out; it is opt-in because correcting it changes what each app believes about who is
  calling. Read the helper's header before flipping another origin.
- **The retired VM (`vm:108` on monster1) must stay stopped, and two things keep it so:**
  HA's desired state is `stopped` (`ha-manager set vm:108 --state stopped`, persistent in
  `/etc/pve/ha/resources.cfg`) **and** `qm set 108 -onboot 0`, because `onboot: 1` would let
  a monster1 boot start it before HA reconciled. Its MAC still has the live DHCP reservation
  in `common/dns-records.nix`, so booting it takes `.36` back and two hosts answering one
  address is silent for whoever is logging in. Do not delete it or its `/srv/authentik`
  while they are the only copy of the pre-cutover state.
- **Switch logins still do not authenticate over RADIUS, and it is not the move's fault:**
  there is no `agent` user in authentik, so the identification stage recurses and the
  outpost sends *no reply* (`exceeded stage recursion depth`), which is what the appliance
  did too. The switch falls back to its local account database either way.
  `docs/procurve-switches.md`.

- **Before creating an OIDC provider, read `docs/authentik-oidc-notes.md`.** Three
  invisible bites, summarised: copy the flows + `signing_key` from a known-good provider
  (Grafana, pk 3) and match redirect URIs strictly; duration fields must be `key=value`
  (`"minutes=5"`, not `"5m"` — the latter saves fine then 500s at `/application/o/authorize/`);
  a custom scope needs a real **ScopeMapping child row**, a plain `PropertyMapping` is
  silently ignored.
- **Client-side OIDC contract** (what `crates/common-rs/oidc` implements; keep new clients
  consistent): authorization-code flow with **S256 PKCE**, and the challenge must be
  **unpadded** base64url — authentik recomputes
  `urlsafe_b64encode(sha256(verifier)).replace("=","")` and compares equality, so a padded
  challenge fails at token time with `invalid_grant` ("Code challenge not matching").
  Scopes `openid profile email groups`; group claims come from the groups scope mapping,
  not a user profile attribute.
- Admin access: `akadmin` is **disabled**; real admins are in the superuser groups `Infra`
  (or `pgina`), e.g. `cjdell`. The admin REST API is **not usable from the box** (its nginx
  proxies `/api/v3/...` collection endpoints to the UI SPA — they 404 with HTML), so
  everything goes through the `ak shell` ORM.
- OAuth2 providers/apps are managed via the ORM/UI (no declarative config). Client secrets
  are **write-only in the UI** but *readable through the ORM*, so never dump a whole
  provider into a terminal or a log you will keep.
- Provider pks, client ids, secret locations and the group gates: **the table in
  `docs/authentik-oidc-notes.md`** (Grafana 3, gocardless-dashboard 33, filestore 34,
  switch-gui 35, all `Infra`-gated).

## Monster (Proxmox hypervisor, 10.3.1.11)

`monster1` runs Proxmox VE 9.x and hosts VMs that are *not* managed by this flake; nothing
there is declarative. **Root SSH is keyed here** (unlike the flake machines):
`ssh -i ~/.ssh/agent-hop-key root@10.3.1.11`.

- **VM disks live on the NAS** (`nas2-nfs`). If the NAS is still booting after a power cut,
  every start fails with `storage 'nas2-nfs' is not online`: check `pvesm status` /
  `mount | grep nas2-nfs` **before blaming the VMs**, and do not treat failed start tasks
  as a VM-level fault. Boots take many minutes — that is pool latency.
- HA-managed VMs (108 authentik, 109 apps1, 127 web1, 128 mercury) start with
  `ha-manager set vm:<id> --state started`, **never** `qm start`. Clearing `error` state is
  **disable → wait → start**; doing both in one loop races and the start is rejected. The
  non-HA autostart VMs (107 mx1, 132 Terraria) are `qm start`ed by hand.
- Leave the `onboot: 0` lab VMs alone: ~24 GB of the host's 32 GB is already committed by
  the six infra VMs.
- Full procedure after an outage: **`docs/monster-powercut-runbook.md`**; inventory and
  storage: `docs/monster-proxmox.md`.

## SSH between machines (machine-hop-key)

- Every flake-managed machine shares the **machine-hop-key**: the same private key at
  `~/.ssh/agent-hop-key`, with the public key authorized for `leigh-admin` in
  `common/users.nix`. No ssh-agent is running, so pass it explicitly:

  ```bash
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20   # services1
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.32   # aibox
  ssh -i ~/.ssh/agent-hop-key root@10.3.1.11          # monster1 (Proxmox; root is keyed)
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.1    # router (root is NOT keyed; sudo)
  ```

  The router's line used to read `root@10.3.1.1` and "root is keyed", and it was wrong
  in exactly the way that costs a day: `just router-known-hosts` was written from it and
  could not reconnect after the cutover, because the OPNsense box's root *was* keyed and
  `gw`'s is not (`prohibit-password`). Verified both directions on 2026-10-21:

- Connect as **`leigh-admin`, never `root`** on the flake machines — root SSH is not keyed
  there (verified both directions). `leigh-admin` has passwordless sudo, so run
  `sudo nixos-rebuild ...` / `sudo nixos-confirm` after logging in.
- **Check `hostname` before you hop — you may already be on the box you are hopping to.**
  Every flake machine holds this repo and its own copy of the hop key, so a session that
  started on `gw` and types a hop to 10.3.1.1 has already succeeded before it wonders: the
  nested shell answers 0 and prints plausible output while keeping none of the cwd, the
  `git log -1`, or the confirm-chaining that the outer shell had established. `just deploy
  <machine>` is the form that is right wherever it is typed (it compares `hostname` to the
  machine and switches *here* when they match); hand-written ssh chains are where the
  mistake lives. Rule 27, and the receipts: `docs/nix-dev-workflow.md` ("Which box am I on?").
- **services2 is no longer the exception** (verified 2026-10-15): it has the private
  `~/.ssh/agent-hop-key`, identical to services1's, and hops out to 10.3.1.20. What is still
  true is that it has **no GitHub key of its own**, so reach GitHub from it with `ssh -A`
  from services1 (agent forwarding — the key must be *in* the agent, `ssh-add ~/.ssh/id_ed25519`;
  check `ssh-add -l` before suspecting the forwarding). Do not copy a private key onto a box
  as a side effect of a deploy.
- **Git from a box with no key of its own: use agent forwarding.**

  ```bash
  ssh -A -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.21 \
    'cd ~/Projects/infrastructure-nix-flake && git fetch && git reset --hard origin/main'
  ```

  Two things must be true and neither is the default: **the key GitHub accepts has to be in
  the agent** (`ssh-add ~/.ssh/id_ed25519` first — check `ssh-add -l` before suspecting the
  forwarding, because a forwarded key GitHub rejects is indistinguishable from no key), and
  **the connection must pass `-A`**. `ssh -T git@github.com` from the remote is the
  one-line test; a `git bundle` from services1 is the fallback.
- Gotcha: on aibox, `/etc/hosts` maps `aibox` to `127.0.0.2` (self reference), so use the
  IPs rather than hostnames.

## Secrets

- Secrets are sops-encrypted in `secrets/secrets.yaml`. sops-nix renders the runtime ones
  to **`/run/secrets/<name>`** (root:secrets, `0440`); `machines/services1/sops.nix`
  declares which ones exist and `common/config.nix` holds the `*_FILE` paths services
  read (`ENV_FILE = /run/secrets/env_file`, …). **Reference the paths; never read
  a secret's contents into a transcript, and never print a token.** (Older notes here said
  `/var/lib/secrets` — that path is gone.)
- **`config.sopsSecretText` is the build-time path, and it is gated by name.** What it
  returns is a **world-readable store path**, present on every box that builds or
  substitutes, and it decrypts the *whole* `secrets.yaml` to extract one value. A name may
  therefore go through it only if it is in `infra.buildTimeSecrets` (`common/sops.nix`)
  with the sentence saying why the secret must be *inside* a generated file — a systemd
  unit, a rendered config, an initrd — rather than opened from `/run/secrets` at runtime;
  the eval throws on a name that is not listed, and on a name that is not a bare
  identifier. Each listed name is a credential in the store forever, by name. The comment
  this replaced listed "the exceptions" in `machines/services1/sops.nix` and had drifted to
  seven of ten — a comment listing exceptions is a list nobody updates — which is why the
  list is an option with an assertion now (audit §4: `docs/flake-simplification-audit-2026-10-22.md`).
- **What build-time decryption costs, in one place.** The secret boundary is *not*
  `root:secrets`; it is **any code that runs during a build, on any machine that takes a
  build**: the age key sits at `0440 root:nixbld`, `/var/lib/sops-nix` is in
  `extra-sandbox-paths`, and all three delegate builds to aibox, which holds the same key.
  (`sandbox` is **not** part of this: services1 runs `relaxed` for one package whose build
  wants the network — `door-entry-management-system`'s `deno i` — and services2 is on the
  default again, measured rather than mirrored.) Those tmpfiles rules are a
  **repair, not an install** — a `z` rule does nothing when the path is absent (verified
  against an `f` rule on gw), so `just install-sops-key` is the mechanism and skipping it
  fails much later as a sops error inside an unrelated build. And each name is its own
  `runCommand` decrypting the whole `secrets.yaml` for one key, all of them keyed on the
  file: change `slack_url` and every unit that embeds any secret is dirtied and
  restarted. That is the reason the runtime model is the default and this list is short.
- Prefer `EnvironmentFile` on the unit so a `DynamicUser` service never opens the secret
  file (the pattern `switch-gui.nix` uses) over a service reading `CONFIG.ENV_FILE` itself.
- OIDC client secrets are write-only in the authentik UI and readable through the ORM:
  generate your own, pass it on create/save, store it in sops, and never dump a provider
  object. The RADIUS shared secret was leaked into a transcript on 2026-10-12 this way —
  rotation is an open item in `docs/follow-ups.md`.

## Service-level rules that are security rules

- `--no-auth` (treat every request as a local session) is **only honoured on a loopback
  bind**, and the gate parses the host as an `IpAddr` and asks `is_loopback()`. It used to
  end in `host.starts_with("127.")`, which accepted `--bind 127.example.com` — a hostname
  anyone with a DNS zone can point at a routable interface — and the flag's *only* guard
  was gone. `common_service::is_loopback` carries the tests (`127.example.com`,
  `127.0.0.1.evil.com`, `::1234`). `--dev-user <name>` is a second test hook and is never
  passed in production.
- **`X-WEBAUTH-USER` is an identity assertion, and "who may send it" is the boundary.**
  `switch-gui`, `status-dashboard` and `filestore` read the signed-in user from that header
  and trust it because the app binds loopback — the guarantee stated in
  `crates/switch-gui/src/routes.rs` as *"there is no path to this socket that does not go
  through nginx"*. Loopback is a boundary; a LAN address is not, and podman publishing on
  `0.0.0.0` means "the app is loopback" is a per-service fact to check (`ss -lntp`), not an
  assumption. So under the front-door plan no origin moves off loopback until the
  origin-reachability rule (reachable from `10.3.1.1`, nothing else) ships with it, and an
  app that genuinely must move takes a shared secret in place of the header's implicit
  trust rather than an exception to the rule.
- **filestore path containment:** `Store::resolve` rejects any `..` component and the root
  is canonicalised at startup, but symlinks **inside** the share are followed wherever they
  point (the NAS uses them). The guarantee is "no traversal *from the API*", not "nothing
  outside the share is reachable".
- Media responses carry `nosniff` and `default-src 'none'` CSP: an `<img>` cannot run
  script, but the same URL opened as a document can — that is the SVG case.
- **Switches: nothing writes NVRAM** unless someone asks explicitly.
- `crates/network-status/frontend/dist/` is **committed** and copied into the store; the
  four Dioxus `dist/`s are git-ignored and built by the flake. Editing the SolidJS SPA
  without rebuilding and committing the dist ships the old UI.

## Known traps worth a sentence (detail in the linked doc)

- **`CREATE DATABASE` can fail with a `template1` collation version mismatch** after a
  glibc upgrade; `ALTER DATABASE template1 REFRESH COLLATION VERSION` on the maintenance
  DBs is the fix, and the application DBs still record the old version.
  `docs/work-log-2026-10.md`.
- **A service's first start can legitimately fail** when it races a setup unit that creates
  its role/directory (e.g. `switch-gui` vs `postgresql-setup`); the never-give-up policy
  brings it up seconds later. Not a fault — read `Result=` and the second attempt.
- **`switch-gui` needs the database to start at all**, and its DSN is the unix socket in
  its percent-encoded host form (`postgresql://…@%2Frun%2Fpostgresql/…`); libpq's socket
  form parses to an empty host and sqlx refuses it. The shipped default was once the
  refused form and the unit could not start — `the_default_dsn_is_one_sqlx_can_parse`
  keeps it true.
- **A UI that refuses an edit must say so.** A refusal that only appears in a banner above
  fifty rows, or only in the journal, is indistinguishable from a dead button — and the
  form should not offer a state the server will refuse.
  `crates/switch-gui/README.md`.
- **Strata's 3060 pays a half-precision tensor cap the *driver* imposes, and the third-party
  kmod that opens it is prefill-only and boot-critical**: it has to load in the initrd before
  `nvidia` binds, its blobs come out of NVIDIA's proprietary package (not a flake input, and
  never one), and it leaves decode untouched — which is where this box's time actually goes.
  Upstream's own numbers make it a 1.2-1.3x prefill ceiling here, and one cuBLAS GEMM on the
  card decides it without touching boot: `machines/services2/STRATA.md` + `docs/follow-ups.md`.
- **Proving a component end to end is not proving the system**: the switch write-path fix
  was verified by driving the console directly, which is why it passed while the queue
  swallowed every change for a day. Test through the path users take.

# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack
infrastructure.

## Layout

```text
crates/        every Rust crate; one cargo workspace (crates/Cargo.toml)
  common-rs/     the shared in-tree crates (json, oidc, web, pg, service, …)
  <program>/     one directory per service or tool; its Dioxus SPA in frontend/
                 (six are not workspace members: the five SPAs' `frontend/`s,
                 which are wasm, and `whisper-ws/`, which is packaged on its own)
common/        Nix modules shared by every machine (crane helpers, hosts, nginx)
               (every file in here starts by saying what kind of file it is:
                `MODULE` a NixOS module · `DATA` a plain table read by `import`
                · `HELPER` a function modules call (arriving as INFRA/CRANE, or
                imported) · `CHECK` a plain-eval assertion file · `PACKAGE` a
                package `callPackage`d by path — `grep -l '^# DATA' common/*.nix`)
machines/      one directory per machine; services/ one file per service
               (`pi-room-sys/` is the image aibox netboots the Pi Room with, so it
               runs on no box of its own, and `gw/` **is** the network's router —
               live at 10.3.1.1 since 2026-10-21, machines/gw/README.md)
secrets/       sops-encrypted
docs/          reference docs (services, builds, DNS, switches, backups) plus the
               write-ups, post-mortems and audits the backstory points to
```

A name that exists in `machines/<m>/` **shadows** the same name in `common/` —
`import ./containers.nix` inside `machines/services1/` means that machine's file, while
the `./common/containers.nix` in `flake.nix` is the other one. `containers.nix`,
`int-dns.nix`, `networking.nix`, `configuration.nix`, `status.nix`, `strata.nix` and
`sops.nix` all exist on both sides of that rule.

Build anything in `crates/` with `cargo build -p <name>` from there; `just test`
and `just clippy` run the whole workspace. `AGENTS.md` is the rules file — the
detail it points at lives in `docs/` (services, builds, DNS, switches, backups,
follow-ups).

## Where a fact lives

The question a first change has to answer is "where does this setting come from", and
the answer is a five-layer chain: `flake.nix` (the machine list, the inputs, the
shared module list) → `machines/<m>/default.nix` (hostName, build farm, the import
list) → `machines/<m>/configuration.nix` (stateVersion, kernel, what differs) →
`common/base.nix` (timezone, bootloader, sudo, autoRollback) →
`machines/<m>/services/*.nix` (the apps). All five can define the same option, and
`flake.nix`'s module list is the only place that says which of them a machine gets.

So: one fact, its owner, the other places it is written, and what catches a mistake.
Three rows say `nothing`, which is the honest answer and the reason the check cells
matter.

| Fact | Owned by | Also written in | Caught by |
| --- | --- | --- | --- |
| A machine's address | `common/hosts.nix` | `common/dns-records.nix` (the reservation) and `common/vlans.nix` (`ipv4`, for the router) | `machines/gw/lib/check-network-tables.nix` — one direction only: every `hosts.nix` address is answered by *some* reservation |
| The NAS address | `common/hosts.nix` (`nas`) | `common/nas.nix`'s `infra.nas.host` **default**, `"10.3.1.6"`, which nothing anywhere overrides | nothing |
| A `*.int` vhost name | `common/int-dns.nix` | ~150 literal `int.leighhack.org` strings across ~45 `.nix` files | `machines/services1/int-dns.nix`: services1 refuses to evaluate unless that table equals the `*.int` vhosts it serves |
| The zone suffix | `CONFIG.LOCAL_DOMAIN` (`common/config.nix`) | the same literals | the router's checks use `LOCAL_DOMAIN`, so the literals are the drift; `docs/follow-ups.md` owns the conversion |
| Which devices exist and keep an address | `common/dns-records.nix` | — | `machines/gw/lib/check-network-tables.nix` |
| Which VLANs and blocks exist | `common/vlans.nix` | the ACL in `common/config.nix` (as nginx text, re-parsed by `machines/gw/lib/network-tables.nix`) | the same check |
| A runtime secret's path | `common/config.nix` (the `*_FILE` options) | per-service `CONFIG.X` reads, and `machines/<m>/sops.nix` | nothing — a typo is a missing `/run/secrets/<name>` at runtime |
| A build-time secret's name | the files that `builtins.readFile` a `config.sopsSecretText` path at eval time | the header comment of `machines/services1/sops.nix` | nothing (and reading a derivation at eval time is what makes `just check` machine-dependent — `docs/flake-simplification-audit-2026-10-22.md` §4) |
| Which services run | `machines/services1/services/default.nix` | `docs/services.md` (by crate, not by machine) | `nix flake check`, for the *imported* ones only — a parked module is not evaluated at all |

## The four commands

```bash
just check      # the gate: evaluates everything, formats, builds NOTHING
just fmt        # alejandra over every tracked .nix (the only formatter)
just switch     # build + apply + sudo nixos-confirm, chained, on THIS machine
just deploy m   # the same, on machine `m`: ssh in, fetch the commit, build and confirm THERE
```

`just switch` resolves the machine from **`hostname`** — nixos-rebuild-ng's default attr is
this box's name — and a session may be running on any box of the fleet, so read `hostname`
before anything sshs: every flake machine has this repo and the hop key, so a hop to the box
you are standing on succeeds silently into a shell that kept none of the state you had read
outside it. `docs/nix-dev-workflow.md` ("Which box am I on?") and AGENTS.md rule 27.

Two things bite, and both are load-bearing:

- **`just check`, not `nix flake check`.** The bare flake idiom *builds* — the
  devShells, `packages.strata`, `strata-cuda` and `whisper-ws`, ~10 minutes on a lock
  bump — and a build on `gw` is a compile on the box that answers DNS for the network.
  `just check` runs the standalone assertion evals, `nix fmt -- --check`, and
  `nix flake check --no-build`. **That last step is not eval-only in practice**: evaluating
  the machines reads a derivation (the crane SPA source stubs — the lock-file half closed
  2026-10-25), so on a flake source nothing has evaluated yet it needs one trivial source
  copy, and `--no-build` refuses to do it. Which means it fails on *any* box with a cold
  store for that source, and permanently on `gw`, which refuses builds by design — so `just
  check` **skips it on `gw` and prints why**, and `just check-eval` (the evals + format, no
  machine evaluated) is the router's real gate. A green `just check` on `gw` therefore covers
  less than a green one on aibox; say so when you quote one. `docs/nix-dev-workflow.md`,
  "Closed 2026-10-25", has the class, the honest detector, and the decision not to buy the
  dependency wiring that would close it. The build-shaped recipes
  (`just check-build`, `just test`, `just clippy`, `just build-frontend`,
  `just filestore-*`, `just switch-gui-test`, `just network-status-test`, `just build-remote`) refuse to run on
  `gw` via `_build-host-guard`. `nix develop` itself now builds nothing — the pinned
  wasm-bindgen-cli that the five Dioxus bundles need lives in `nix develop .#wasm`,
  which is a cargo build on a cold box — so `nix develop` at least is safe on the router.
- **Confirm in the same shell invocation as the switch.** Every deployable machine
  sets `system.autoRollback.enable = true`: activating arms a five-minute timer that
  reboots into the last *confirmed* generation. `just switch` chains
  `sudo nixos-confirm` with `&&` for exactly that reason; a confirm in a later session
  can mark the old generation good, and a confirm of a failed activation disarms the
  only safety net. `readlink /run/current-system /nix/var/nix/profiles/system-good`
  prints equal paths when the box is safe.

Then, when anything is built rather than just evaluated, `just check-build` (`nix
build` of all four deployable machines' system toplevels — compiles everything,
activates nothing, writes no boot entry; it is *not* `nixos-rebuild dry-build`, which
builds nothing). Budget ~75 min for a nixpkgs bump.

## Applying Config

The rest of the recipes, beyond the four above:

```bash
just boot               # Build for next reboot (chains sudo nixos-confirm)
just switch-netboot     # Rebuild/switch the netboot setup and restart clients
just deploy gw          # Any machine deploys itself: ssh in, fetch, switch, confirm there
just hard-reboot        # sysrq hard reboot — no orderly shutdown

sudo nixos-confirm      # Mark this generation as good so we don't get rolled back
list-generations        # The last "good", the current, and what was booted
```

## Machines

### Services 1

Path: `machines/services1`

Various hackspace services for members. See
[README.md](machines/services1/README.md).

### AI Box

Path: `machines/aibox`

Experimental machine with LLMs and Stable Diffusion, and the network's build machine
since 2026-10-17. Always hosts the netboot server for the Pi Room computers.

### Services 2

Path: `machines/services2`

Strata's CUDA engine on the RTX 3060, plus its own status dashboard. Its RAM is the
engine's; it stopped being the network's build machine on 2026-10-17. See
[README.md](machines/services2/README.md) and
[STRATA.md](machines/services2/STRATA.md).

### The router

Path: `machines/gw`

**The** router since 2026-10-21, at 10.3.1.1: dnsmasq and nftables generated from the
shared network tables, the AAISP tunnel, the tailnet client. It leases every address in
the building, so read [README.md](machines/gw/README.md) before changing anything here.

### The Pi Room netboot image

Path: `machines/pi-room-sys`

A real `nixosConfiguration` that runs on no box: aibox builds it and serves its kernel,
initrd and store over iPXE + NFS. See [README.md](machines/pi-room-sys/README.md).


# services3

The sixth configuration in this flake and the fourth deployable box. **On the bench, on
Wi-Fi, and since 2026-11-21 the network's third Strata engine** — Qwen3.8-Flash-Next
Coder on the GTX 1080 Ti, `strata3.int.leighhack.org`. It was llama.cpp (Vulkan) from
2026-11-12 to that day; one GPU cannot serve two engines, so `./llama.nix` is parked and
its measurements are kept below under "History".

This file owns the measured hardware, the two things that make the box different from
the other services boxes, the engine's sizing on *this* card, the move to VLAN 225, and
what was deliberately left behind when it came off its previous fleet.

Nothing here is a guess unless it says it is. Everything under "Measured" was read off
the box on **2026-11-11** through the account it arrived with, at 10.3.2.213 over a Wi-Fi
dongle, and re-read on **2026-11-25** after it took a cable onto wired VLAN 225; the
previous owner's write-up is quoted only where it is the *only* record (and then as
quoted, not as fact). The 2026-11-11 read is still what the hardware rows below say about
the board and the card, because none of that moved; where a row says something about an
address, an interface or an uplink, read the 2026-11-25 "The move to VLAN 225" section.

## Measured

| | |
| --- | --- |
| Board / CPU | ASUS Z170 PRO GAMING, i7-6700 (Skylake 4c/8t, `kvm-intel`), **40 GiB RAM** — `MemTotal: 41999736 kB`, measured 2026-11-21. Earlier versions of this file (and of ./hardware-configuration.nix) said 16 GiB, which is how the box arrived and was never re-measured; the difference is what makes the Coder pack fit |
| RAM / DIMMs | **40 GiB of a 64 GiB board maximum, on four slots, all four full** — `sudo dmidecode -t memory` 2026-11-22: `Maximum Capacity: 64 GiB`, two `Kingston KHX2400C12D4/4GX` **4 GiB** DDR4-2400 and two `Kingston KTP9W1-MIE` **16 GiB** (all four seated, `MemTotal: 40999748 kB` read today. The Board/CPU row above records `41999736 kB` on 2026-11-21, which is **above** what 40 GiB of DIMMs can report once the kernel and firmware take their reservation, so that older figure is the suspect one — 1 GiB apart, two reads, and the open item says which is which (unless a stick was changed on the bench between the two and nobody wrote it down). So **64 GiB is a purchase, not a rearrangement**: the slots are full and the headroom is two new 16 GiB sticks, with the two 4 GiB ones as what gets thrown away. `dmidecode` is in `common/tools.nix`, so this is one command on any box, not a guess. The sibling box cannot take RAM at all — `machines/services2/STRATA.md` and `docs/follow-ups.md` read its board as *two* slots, both filled with 16 GiB DDR4-3200 (M378A2K43DB1-CVF) at 2933, max 128 GiB, so its own headroom is a 2×32 swap. And its RAM must not be robbed for this one: `setup.py`'s `low_ram_needed` verdict and the KV trade are what make the Coder pack fit there at all | 
| GPU | **exactly one, and it is a GTX 1080 Ti (GP102/Pascal on `01:00.0`** (+ its HDMI audio function `01:00.1`), driven by this flake on `nvidia 580.178.04` — ./nvidia.nix. Re-measured 2026-11-12 because "why is the 1060 missing?" was asked: `lspci -nn -d 10de:` lists those two functions and nothing else, NVML reports `GPU 0` only with `/dev/nvidia0` the sole card node, and `llama-server --list-devices` prints one line. **There is no second GPU in this box.** The network's GTX 1060 lived in **aibox** and was physically removed on 2026-09-02 (its llama.cpp verdicts were negative; the write-up is in git history) |
| No iGPU either | `lspci -nn \| grep -ciE 'intel.*(vga\|display\|3d)'` = **0** — the i7-6700's HD Graphics 530 has no display function on this bus (disabled in the firmware), which is why `vulkaninfo` shows the NVIDIA ICD and Mesa's `llvmpipe` and nothing in between. An earlier version of this file and of ./llama.nix asserted the iGPU was a second Vulkan device; it had never been measured |
| Free slots | DMI slot table: `PCIEX16_1` **In Use**, `PCIEX16_2` **Available**, `PCIEX16_3` **Available** (both reported "Short" — the board's electrically-x4 slots), `PCIEX1_1` Available; and the kernel log has no BAR/MMIO refusal, so nothing is seated-and-refused. A second card could therefore be fitted. What that changes is `vulkanDevice` in ./llama.nix (the index order is ICD load order, **not** PCI order) and, if the intent is one model across two cards, the `--device` list — not the driver, and not this table's VRAM ladder |
| Disk | `sda` 111.8 GiB: `sda1` 1 GiB vfat (label `BOOT`) + `sda2` 110.8 GiB ext4 (label `ROOT`) — the OS, the store and the generations. `nvme0n1` **476.9 GiB** (SK hynix PC601): one ext4 filesystem labelled `STRATA` since 2026-11-21, mounted at `/srv/strata`, holding the model data. It arrived with a previous owner's two-partition ZFS pool (`dbthr33`) and an 11.9 GB swap partition, neither referenced by this tree nor mountable here (no ZFS), so the whole disk was wiped — ./hardware-configuration.nix carries the decision |
| Boot | UEFI 2.40, systemd-boot, Secure Boot off, no TPM. `mitigations=off` on the live command line |
| Swap | **none**, and none added for the engine: the arena is RAM-resident (`--resident-experts`) and a swap file would sit on the same NVMe that serves the PLE table. ./hardware-configuration.nix has the argument |
| Wired NIC | `enp0s31f6`, Intel I219-V, MAC `70:8b:cd:a8:50:bc` — **cabled, and the cable is live**: `ip -br link` reads `UP ... <BROADCAST,MULTICAST,UP,LOWER_UP>` (a carrier, which is what changed since the 2026-11-11 read of `DOWN ... <NO-CARRIER>`) and `ip -s link` counts RX 1045 packets / TX 120, so the far end is a network that talks. That MAC is what `./vlan225.nix` matches and what `common/dns-records.nix` reserves |
| Wi-Fi | `wlp0s20f0u1`, MAC `90:de:80:e1:4e:42`, Realtek RTL8821CU (`0bda:c811`) on `rtw88_8821cu` — **hardware still in the box, not an uplink any more**: `./vlan225.nix` forces NetworkManager off, so the interface is unconfigured. The SSID `WiFi Go Brrr` and its PSK live in `/etc/NetworkManager/system-connections/WiFi Go Brrr.nmconnection` (root, 0600), which this flake never managed and still does not; the dongle is a drawer spare |
| Address | `10.3.1.22/24`, gateway `10.3.1.1`, DNS `10.3.1.1`, search `int.leighhack.org` — **VLAN 225**, the wired services block, like every other flake machine. Reserved by the wired MAC above; outside VLAN 225's dynamic range (10.3.1.150–.253), so the reservation costs the pool nothing |
| IPv6 | taken by router-advertisement/SLAAC inside VLAN 225's advertised range: measured `2001:8b0:1d14:225:728b:cdff:fea8:50bc` + the ULA twin, i.e. derived from the NIC MAC, so it is not a number this row can pin — hence `ipv6 = []` in `common/dns-records.nix`, the same honest reason services2's list is empty |
| Reachability | every box dials `10.3.1.22` now, in the wired block; the pinned host keys in `common/ssh-known-hosts.nix` carry that address (the table reads `HOSTS.services3`, so the move followed the row, and `just known-hosts` re-scans it). The old dongle lease `10.3.2.213` answers nothing in this flake's opinion |
| Host keys | `ed25519` + `rsa`, both dated Aug 2025 — the box's own, unchanged by this flake and pinned in `common/ssh-known-hosts.nix` (keyed to the *address* string, so the entry moved from 10.3.2.213 to 10.3.1.22 with the row) |
| OS | NixOS 26.11 (Zokor) on kernel 6.18.63 / nvidia 580.196.07 as of 2026-11-21; at arrival (2026-11-11) nixpkgs `26.11.20261017.09063bd`, kernel 6.18.57, nix 2.34.9 |

## What it runs

**Strata** — `./strata.nix` (service + engine flags) and `./strata-package.nix` (the
CUDA 12.8 / sm_61 build), serving the **Coder IQ1_M** pack on the GTX 1080 Ti at
`10.3.1.22:8080`, behind the door name `strata3.int.leighhack.org`. It is the
network's third Strata instance and the same model file services2 serves. The
sizing, the VRAM budget, the exact flags and the hand-run pack preparation are in
`./strata.nix`'s header; `docs/services.md` has the contract table and the four
checks that answer something.

Nothing else on this box competes for the card or the RAM. `common/base.nix` and the
shared `commonModules` (sops, podman, crane args, build delegation to aibox, the NAS
client with no exports declared, the tool set, the users) are unchanged from every
services box, `common/exporters.nix` puts `node`/`systemd`/`smartctl` on
`:9100`/`:9558`/`:9633`, and `programs.pi-agent.enable` is config plus a timer. The
`llama-server` unit that used to be the box's job is gone from the module list and
`./llama.nix` begins with its PARKED notice.

What this box pays for itself now, and why it matters more here than elsewhere: since
2026-11-23 nothing delegates (`common/nix-gc.nix`'s header), so every build is local —
8 threads of Skylake, and a Wi-Fi dongle as the only uplink, so a compile is work done by
the machine that is also carrying its own ssh, and a CUDA engine build is exactly
that kind of compile (nvcc, ~30 minutes of device code). Before that date the build went
to aibox and only the delta crossed the dongle; now the whole compile happens here, so
**budget it**: a `strata` input bump or a nixpkgs bump on this box is a window, not a
`just switch`, and `systemctl stop strata` first (its ~23 GiB of resident experts is RAM a
compile wants and nothing caps this box's `nix-daemon`). Substitution is unaffected, so a
config-only switch still costs what it always did.

## The two things that make it different from the other services boxes

1. **Its uplink is a USB Wi-Fi dongle on the wireless VLAN.** `networking.nix` explains
   why NetworkManager stays and `./vlan225.nix` is parked. The consequence that matters
   operationally: **this box's address and its route to the network are the dongle's**,
   so the reservation in `common/dns-records.nix` keys on the dongle's MAC
   (`90:de:80:e1:4e:42`) and not the board's wired one. Any change to `networking.nix`
   that does not keep NetworkManager owning `wlp0s20f0u1` strands the box: there is no
   cable, no serial console, and the switch port it would land on is not yet a trunk.
   A switch of this box is therefore made **on** the box, in one chained invocation
   (GOLDEN RULE), and never from a second machine's `--target-host`.
2. **It is not in the power-cut plan.** `machines/gw/ups.nix`'s `shutdownTargets` is
   `services1, services2, aibox` and this box is not in it, because nothing has been
   established about what it is plugged into — a bench box on Wi-Fi is not known to be
   on the UPS the router watches, and putting it in the list would be a claim that the
   router will orderly-shutdown a machine nobody has confirmed is on battery.
   Re-verify with: on gw, `journalctl -u ups-fleet-shutdown` after a test run, and at
   the bench, which socket strip the PSU lead is in. It goes into
   `shutdownTargets` (in the right order — see `docs/ups-and-fleet-shutdown.md`) as part
   of being racked, not before.

## The move to VLAN 225 — **done 2026-11-25**

The plan below is what was decided on the bench in 2026-11-11; what actually happened on
2026-11-25, and the four things that differed from the plan, are in this section. Both
halves are kept because the order is the part that can strand the box.

### What was measured before any of it, and what each reading rules out

| reading (on the box, 2026-11-25) | what it proved |
| --- | --- |
| `ip -br link` → `enp0s31f6 UP ... LOWER_UP`, MAC `70:8b:cd:a8:50:bc` | a **carrier**: the cable is in, which is the change since 2026-11-11's `DOWN ... <NO-CARRIER>`; and the MAC is exactly the one `./vlan225.nix` matches, so no re-measure of the board was needed |
| `ip -s link` → RX 1045 packets / TX 120 | the wire is **live**, not a dead conduit with a cable in it |
| NetworkManager's own journal, repeated today: `Activation: failed for connection 'Wired connection 1'` | the cabled NIC could not get an address **untagged**: every DHCP offer the router makes is `tag:`-qualified per VLAN, so an untagged request in the wired block gets nothing. This is why NM's failure is not a fault to chase and is the thing VLAN tagging answers |
| a VLAN-225 child configured against the **real** NIC name, then `vlan225: DHCPv4 address 10.3.1.248/24, gateway 10.3.1.1 acquired from 10.3.1.1` | **the switch port is a trunk.** A lease out of VLAN 225's own dynamic range, whose gateway the router names as itself, only happens if VLAN 225's tag arrives down that cable. This is the one physical fact the flake cannot state, and it is why the experiment ran before the deploy rather than after it |

### What was done, in the order it was done

1. `common/hosts.nix` → `10.3.1.22` (outside VLAN 225's dynamic range 10.3.1.150–.253,
   so the reservation costs the pool nothing), `common/dns-records.nix` → `vlan = 225`,
   wired MAC, `ipv6 = []`, `machines/services3/vlan225.nix` unparked, and
   `machines/services3/networking.nix` **deleted** — one commit, because
   `machines/gw/lib/check-network-tables.nix` refuses a `hosts.nix` address no
   reservation answers and refuses to let a machine have two addresses' worth of rows.
   The dongle's MAC left `common/dns-records.nix` in that same commit, as the plan's step
   5 asked for.
2. `just check` / `just check-eval` on services1 (the guard runs from both), then push.
3. `just deploy gw` first — the reservation has to be the router's before the box asks
   for its lease (GOLDEN RULE confirmed in the same invocation, then
   `sudo systemctl start front-door-cert-check.service` and read the *issuer*, because
   that is the only check that tells a CA-issued certificate from minica's fallback).
4. services3 switched and **booted** there.

### What differed from the plan

1. **A switch is not enough; the box needs a boot.** `systemd.network.links` renames the
   NIC to `lan`, and udev applies a `.link` only when the device is (re)created — so the
   switch wrote `/etc/systemd/network/10-lan.{link,network}` and `networkctl` still
   printed `enp0s31f6 ... unmanaged`, with no `vlan225` device, and the VLAN child never
   existed until the reboot. Measured, not inferred: the interim switch was read back
   before the boot. Do not "fix" this by matching `enp0s31f6` in the flake: a board swap
   or a BIOS rename changes the *name* and not the MAC, so the MAC match plus rename is
   the stable shape, and it is what services1 and services2 already use.
2. **The plan's step 4 assumed the box would have to be switched by hand at the bench. It
   did not**: the experiment above was run with NetworkManager deliberately left **on**, so
   the dongle stayed a working uplink while VLAN 225 was being proven, and the hop to the
   box kept working over the dongle address until the final switch. That is the
   non-stranding order: prove the wired path while the wireless one still works, then
   cut over. Leaving NM on for the experiment also left the two-DHCP-clients-on-one-NIC
   trap live, and it bit in the way the parked notice predicted rather than in a new way.
3. **The plan's re-verify was right, and the reason it looks wrong is the box's own
   `/etc/hosts`**: `getent hosts services3.int.leighhack.org` **does** answer — read from
   `gw` on 2026-11-25 it printed `10.3.1.22  services3.int.leighhack.org`.  `common/dns-records.nix`
   is one dnsmasq record named `services3` inside the zone `int.leighhack.org`, and
   `dhcp-fqdn` hands the same lease out again under `<name>.<zone>`, so the record's name
   exists both bare and qualified; neither form is a row in `common/int-dns.nix` or a vhost
   (`strata3.int.leighhack.org` is that row — rule 24). What the plan would have tripped on
   instead: **run on the box that owns the name, `getent hosts services3` answers `127.0.0.2`**
   — NixOS writes `127.0.0.2  <machine-name>` into every flake box's own `/etc/hosts`
   (measured on all five on 2026-11-25), which AGENTS.md's SSH section has carried as an
   aibox-only gotcha for a year. A box can never ask itself what its own name answers; the
   check belongs on `gw`.
4. **Inbound LAN traffic to the box followed its newest interface, not its old one.** After
   VLAN 225 was up beside the dongle, the hop to `10.3.2.213` stopped answering rather than
   falling back: conntrack on the router read `SYN_SENT src=10.3.1.20 dst=10.3.2.213
   dport=22 [UNREPLIED]` (and the same for two of the box's exporters, while a third stayed
   `ESTABLISHED`). The box was alive throughout — its engine still answered through the door
   — so "the box is unreachable at its old address" is a NAT/VLAN fact, not a dead machine,
   and the way to read it is the router's conntrack table rather than the ssh timeout. Once
   the move completed, every hop is `10.3.1.22` and the dongle answers nothing at all.

## What came with the box, and what was dropped

This machine was a member of **another fleet** — `github:cjdell/nixos-config`, the
`grafton.lan` network — where it was `asus-i7-6700-nixos` (`.65`), a Folding@Home space
heater with a Plasma console, and it arrived at the hackspace on Wi-Fi at a dynamic
VLAN 226 address. "Start afresh" was the instruction, so this flake takes the machine
and not the role. Concretely, a switch to `.#services3` removes:

- `fah-client`, `fah-panel` (which was answering on `:80`) and the folding-heater /
  `gpu-mode` machinery. `:80` stops listening — worth knowing if anything was pointed at
  the FAH panel page.
- SDDM + Plasma 6 + pipewire + firefox and the home-manager state for `cjdell`. The box
  is headless from here on; there is no console GUI any more.
What it **brought with it** and kept: `/home/cjdell/Models/LFM3-1.7B-Q4_K_M.gguf`
(1,099,627,808 bytes) — the only thing on the box worth more than the box. It was
**copied**, not moved, to `/home/leigh-admin/Models/` (the path both Strata engines use,
and the path `./llama.nix` pointed at until the model changed on 2026-11-12), sha256-verified
on both sides; the original is
still there, and removing it is a decision for whoever owns `/home/cjdell`, not a
side-effect of onboarding.

And the old build client (`/etc/nix/machines` → `ssh-ng://root@192.168.49.50`, that
fleet's builder, unreachable from this network, which is why the box was compiling
locally at `max-jobs = 1`) — removed, first in favour of aibox and, since 2026-11-23,
in favour of nothing: the box builds locally and `/etc/nix/machines` is empty.

And it **keeps**, on purpose: the box's own SSH host keys, the `cjdell` account, the
disk, and the Wi-Fi credential file below.

`cjdell` is kept by declaring it — `./users.nix` — and not by luck. The first
`nixos-rebuild dry-activate` of this machine said:

```
would remove user ‘cjdell’
```

because the box's previous flake declared that account and this one did not, and NixOS
removes the difference (with `userdel -r`, so `/home/cjdell` and everything in it goes
with it). The belief that "`users.mutableUsers` is true, so undeclared accounts survive"
is true for accounts NixOS never managed and false for exactly this case; the dry-run is
what found it, before the switch rather than after. `./users.nix` carries the reason, the
uid choice, and the one thing it deliberately does *not* declare (the password, which is
the password-authenticated way back onto a box whose only uplink is Wi-Fi — verify with
`sudo sshd -T | grep -i passwordauthentication`).

## What is off-flake on this box (and who would be surprised)

- **`/etc/NetworkManager/system-connections/WiFi Go Brrr.nmconnection`** — root, 0600,
  made by hand at the bench. It holds the PSK for the SSID this box reaches the network
  through. NixOS does not manage that directory, so no deploy rewrites or deletes it and
  no deploy can reproduce it. **A reinstall loses the box's network access.** Making it
  declarative is described in `networking.nix`; not done, because the box is leaving this
  VLAN on purpose.
- **`/home/cjdell/nixos-config`** — the other fleet's checkout (its own flake, secrets,
  crates). It is not referenced by anything here and `nix flake check` on this box does
  not evaluate it. Deleting it is not this flake's call.
- **`/var/lib/fah-client`** — the folding client's state (DB, cores, logs) and
  `/var/lib/private/…` equivalents stay on disk after the units disappear. They are the
  previous role's data; nothing reads them and nothing removes them. They are ~1 GiB of
  the 84 GiB used.
- **The 77 GiB `/nix/store`.** The old role's closure stays reachable through the old
  system profile generations until someone prunes them. This box has 20 GiB free, which
  is enough for a switch or two but not for a nixpkgs bump *and* a build. Reclaim with
  `sudo nix-env -p /nix/var/nix/profiles/system --list-generations`, keep the one you can
  still want back, then `sudo nix-env -p /nix/var/nix/profiles/system --delete-generations
  <numbers> && sudo nix-collect-garbage -d`. That destroys the rollback path to the
  folding configuration, so it is a decision, not housekeeping.

## The engine (2026-11-21): Strata on a Pascal card

`strata.service` serves **Qwen3.8-Flash-Next Coder IQ1_M** — the same pack services2
serves — with the experts' ~23.4 GiB arena resident in RAM (`--resident-experts`), the
dense weights and the KV in VRAM, and the ~27 GiB PLE table read from the NVMe. The
flags and the VRAM budget are `./strata.nix`; the build shape is
`./strata-package.nix`; the contract table is `docs/services.md`.

Three things about *this* box make it different from services2's instance, and all
three are why the numbers here are not services2's:

- **Pascal, so CUDA 12.** CUDA 13 dropped sm_60/sm_61/sm_70 outright; `nvcc` refuses
  the architecture. This is the only box in the network that builds the CUDA 12 branch
  of the toolkit (12.8, because 12.9's math headers clash with glibc >= 2.43 and this
  box runs 2.44), and the only one that sets `-DSTRATA_EXPERIMENTAL_SM60=ON` —
  upstream's opt-in path for cards below compute capability 7.5, which is the row of
  their hardware matrix the GTX 10 series sits on.
- **11 GB of VRAM instead of 12, and no tensor cores.** Consumer Pascal runs FP16 at
  1/64 of its FP32 rate, so `STRATA_DENSE_F32=1` is on: the dense projections go
  through `cublasSgemm` on FP32 copies. Upstream measured that route as a large prompt
  win on a Tesla P40 (their other sm_61 card); it is *unverified here*, and the run
  that would verify it upstream's way (`gemm_f32_parity`, a tests build) has not been
  made. It is one line to revert.
- **A 4-core Skylake CPU pool.** The experts the card and the RAM-resident copy do not
  cover are computed on the CPU; services2 has 6 cores of Zen 3. Expect prompts to be
  the better half of this engine's behaviour and decode the worse half.

The card check is the first thing to run after any engine move, and it is the only
thing that proves nvcc really emitted sm_61 and the driver will load it:

```bash
STRATA=$(cd ~/Projects/infrastructure-nix-flake && nix build .#strata-cuda12 --no-link --print-out-paths)
$STRATA/bin/strata-device --list-devices     # must say the 1080 Ti, compute capability 6.1
$STRATA/bin/strata-device --selftest
```

### First boot, measured (2026-11-21)

The load takes ~30 s and the log's own plan is the sizing in one place: *"23.4 GiB
experts, 5.1 GiB in the GPU caches, 21.7 GiB RAM copy (1779 lend slots kept)"*,
*"716 MiB of VRAM free with everything loaded"*, a 4096 MiB conversation cache sized
from the free RAM, and a prompt chunk of 8192. Decode runs the **prompt-lookup
(suffix) drafter** — `--lookup-max-t 12`, MTP windows of 4 — which accepted 38 of 45
drafts on the long probe.

| what | measured (32K, first boot) | measured (**128K**, deployed) |
| --- | --- | --- |
| **Decode** | 17.6–21.0 tok/s warm, 10.4 cold | **18.2 tok/s warm**, 11.8 cold — the smaller expert cache costs ~13 % |
| **Prefill** | 471.7 tok/s at 1,830 tokens | **464.6 tok/s** at 1,830; **538.7** at 37,622; **507.8** at 77,612 — flat with depth |
| **Long context** | — | a needle at 60 % depth of **77,612 tokens** was answered correctly (`ZX-4471`, 152.8 s prefill); same at 37,622 (69.8 s) |
| Expert cache | 4,105 slots, ~5.7 GiB | **1,988 slots, 3.82 GiB** (the KV took the difference), 89.6 % hit rate |
| VRAM | 10,447 MiB of 11,264, 716 free | 10,449 at load, **418 MiB free after a 77K request**, 714 at idle |
| RAM | ~26 GiB used of 40 | 29 GiB used, 9 available (page cache for the PLE/GGUF) |

**128K is at the card's edge, not comfortably inside it.** The engine says so itself
after a long request: *"WARNING: only 418 MiB of VRAM free at the start of this request
(below 512 MiB): when the card is this full the driver can page GPU memory to system
memory"*. It held (no `LOW: requests may stall` line, no paging in the log), and the
retrieval at 77K was correct — but the two dials are now known and both are one line:
`--vram-reserve-mib 512` (costs ~512 MiB of cache, buys the headroom back) or
`context = 65536` (halves the KV, ~0.9 GiB). Watch `nvidia-smi` at 59 °C, not 89 — this
is VRAM pressure, not thermal.

For scale: services2's RTX 3060 does ~32 tok/s on the same pack. This card has 92 % of
the 3060's VRAM, no tensor cores, 1/64-rate FP16 and 4 Skylake cores instead of 6 Zen 3
— 16–21 tok/s is the card being used properly, not the card being misconfigured.

**`--spec` is not optional on a native (IQ) pack**, which the first boot proved by
refusing: `strata generate: … is a native (IQ) pack: it needs --native SHARD1, --spec T
(T >= 2) and --prefill CHUNK`. `--native` was given and `--prefill auto` resolves after
that check, so `--spec` was the only missing piece — it is the verify-window width, and
it feeds the suffix drafter, which needs no MTP weights. It is in `./strata.nix`'s base
flag list now, and `useMtp` only adds `--mtp`.

### The RAM tier, measured (2026-11-22) — it is already at its ceiling; the card is not

**Nothing crossed on this count, and nothing will until it is measured**: `--kv-resident
32768` was written into `./strata.nix` on 2026-11-22 and **reverted the same day**, before any
switch of this box carried it — see `git log -- machines/services3/strata.nix` (one commit adds
it, the next deletes it, and no deploy of this box sits between them). So nothing in this
section below is a claim about *its effect*; what is live is the evidence read off the start
that was running, quoted below. The rule the section ends with is the one that made the revert:
the box already writes the top rung of its own ladder, so an argument for going further has to
come from a needle and the answer key, not from the arithmetic in the two lines it quotes.

The engine's own start lines say the RAM half of the tier is done and the VRAM half is not,
on the live unit (engine 0.1.63, `/var/lib/strata/strata.log`):

```bash
sudo grep -E "lendable slots|resident RAM mode|expert cache auto|expert cache hit" \
  /var/lib/strata/strata.log | tail -8
```

| line (2026-11-22) | what it says |
| --- | --- |
| `1690 of the prompt path's 1690 lendable slots keep their experts in RAM too` | **N of N** — nothing is read off the NVMe mid-prompt, so `--resident-budget-gib` and `STRATA_RESIDENT_HEADROOM_GIB` are **inert here** and are deliberately not set. services2 needed both because its box is 30.7 GiB and the KV trade had to come *out of* the expert budget — there the same dial is a 2× swing (562/725 → 1210/1204 tok/s at 8k, `machines/services2/STRATA.md`'s four-arm table); here the prompt path is already covered. Read this line after any change to the start, before believing a benchmark (`machines/services2/STRATA.md`'s rule) |
| `resident RAM mode: 22.83 GiB of experts in RAM (page-locked), 1996 in the GPU cache … (no file reads)` | the full 23.4 GiB arena resident, as `strata.nix`'s header argues; the GPU cache holds 1,996 experts of it |
| `MemTotal: 40999748 kB`, `MemAvailable: 10824476 kB` (idle) | the box has **10.8 GiB of RAM to spend things in** — the one axis it is not tight on |
| `expert cache auto: 4.78 GiB free, 700 MiB reserved (+0 MiB for the draft head, +256 MiB …) -> 1549 slots` | what the card had left when the cache was sized — the 128K int8 K/V is still in **VRAM** (`kv_streaming_wanted` keeps it there once the arena is resident, `./strata.nix`) |
| `decode expert cache hit rate: 68.4 % (112607 hits / 164618 lookups); 54262 more read by the GPU over PCIe … (24.8 % of all 218880 routed)` | a quarter of the experts the decoder wants are fetched **over PCIe**, not held — this is the prize a bigger card cache buys, and it is the same class as the ~13 % decode cost the 128K KV already took above |
| `verify: capturing the 12-token window (607 MiB of VRAM free)` | the mid-request VRAM floor while `--lookup-max-t 12` runs windows 1,2,3,4,5,6,12 — ~90 MiB above the engine's own 512 MiB paging guard |

So: **RAM capacity is a closed door** (four slots full, 64 GiB needs two new 16s, and nothing is
buyable), **the RAM dials are at their best** (N of N, no file reads), and what is left on the
RAM axis is the *tier* of one thing — the K/V. **And the one attempt at that tier was
reverted, so it is not in `./strata.nix`.** What the attempt was and what stopped it:
`--kv-resident 32768` moves ~1.3–1.7 GiB of int8 K/V off the card into pinned host RAM (which
this box has 10.8 GiB of, un-fought — services2's bill for the same pair was its last 1.1–1.6 GiB,
which is why `LowMemory` fires there and `docs/follow-ups.md` keeps that as an observability
decision rather than a fault), and on services2 the pair bought +13 % prefill at 8k with the
answer key unchanged (`machines/services2/STRATA.md`, "256K at full precision, at this box's
speed"). The slot prize here (~1,988 → ~2,900 at ~1.9 MiB/slot) was **derived** from the two
lines above and never measured, and `docs/follow-ups.md` already records a try of the
*neighbourhood* of this change **on this box** — `--kv-unloaded 16384 16384 0 0` alone, and that
with `--kv-resident 16384 0 0` — that left the residency line at `1690 of 1690` unchanged. A
context of ≤ 32768 cells is not streamed at all, so ordinary turns pay nothing either way.
So the honest state of this section is: **the RAM tier is at its ceiling, and the card-side
prize is unmeasured, hence unclaimed**. Anyone who wants it spells it as its own change,
measures it with a needle and the answer key, and reads BOTH lines back — `sudo grep "lendable
slots" /var/lib/strata/strata.log | tail -1` must still read N of N and `expert cache auto:` says
what the cache got — before the sentence goes into a report. `git log -S '"--kv-unloaded"'
machines/services3/` answers nothing, so that try was made at a console, not by this module:
this start has never carried `--kv-unloaded`.

One thing the RAM tier is doing that nobody has read: the conversation cache sizes itself from
free RAM — `conversation cache sized from the free RAM: 3584 MiB for up to 4 parked
conversations` (README said 4096 at the older start, so the number moves with the load) — and it
**is** parking (`parked=2 bytes=1139510732 evictions=0 snapshot_bytes=884619696`) but **`reused_kv_bytes=0`**
at every start read so far. ~1 GiB of host RAM is being held for parked conversations and nothing
has ever been restored from it. What *is* reusing prompts is the other mechanism — `prompt 38569
tokens = 37990 reused + 579 read in 2022 ms … 4 checkpoints` (`--prompt-cache`). The engine also
says `conversation cache: no SSD tier: no data folder (--pack is not under <data>/packs;
--conversation-spill-dir sets one)` — services3 is the only box in the network with an SSD and
RAM to put that tier on (`machines/services2/STRATA.md`'s answer there was "not with swap"). Open
item, in `docs/follow-ups.md`; deliberately unchanged this change.

### One stability fact this box earned the hard way

**The box crashed once during this bring-up** (2026-11-21 17:50, ~30 min into a 58 GB
download): three kernel Oopses, all `general protection fault … kmem_cache_alloc_noprof`,
all at the same faulting address, one bit away from a canonical pointer, in a
`buffer_head` allocation that `curl` and then `jbd2` were making. The filesystem wedged,
journald hit its watchdog, and the box went down. No NVMe error, no MCE, no ECC on this
board (`EDAC ie31200: No ECC support`) — so nothing can tell "bad RAM" from "kernel
overwrote its own freelist", and both remain open. It did **not** reproduce in the 90
minutes of downloading, pack preparation and a full model load that followed (16 GiB of
`memtester`, one pass, was clean too, which is weak evidence and not a clearance).

If it recurs, the cheap discriminators, in order: `slub_debug=F` on the kernel command
line (it names the corrupting cache instead of faulting), then this flake's pin also
carries **6.12 LTS** (`boot.kernelPackages = pkgs.linuxPackages_6_12`) — the running
6.18 is newer than both the fault and the evidence. Only after those does it become a
repair job: reseat/replace DIMMs, and a `memtester` loop long enough to matter.

The model data is at `/srv/strata` (the wiped NVMe) and is prepared by hand once —
`./strata.nix`'s header holds the exact commands (`iq_pack.py`, then the MTP tools if
`useMtp` is ever turned on). A **missing pack does not crash-loop**: the unit has
`ConditionPathExists` on `index.txt`, so it stays inactive rather than restarting
forever, and the door answers 502 until the pack exists.

## History: llama.cpp (Vulkan), 2026-11-12 → 2026-11-21 (parked)

What follows is the record of the box's first engine, kept because it is the only
measurement of this card as a *compute* device before Strata, and because the
llama.cpp packaging decisions in it (why nixpkgs' expression builds the input's tree,
why the web UI is built with it) are reusable the day something else wants a Vulkan
engine here. `./llama.nix` is parked with this notice at the top of the file, and
un-parking it means un-parking it *instead of* `./strata.nix` — one GPU, one engine.
The name `llama.int.leighhack.org` went with the unit: its door row is now
`strata3.int.leighhack.org`.

One service, `llama-server`, from the flake's **`llama-cpp-upstream`** input — the second
llama.cpp in this flake, and the comment on that input in `flake.nix` is the reason it
exists (the model that first made it so, LFM3, declares `general.architecture = lfm3`; the
fork this network already carries is llama.cpp 0.3.0 and nixpkgs' `llama-cpp` was 0.6.0, and
neither had that arch — **that second half has since expired: nixpkgs ships 0.9.0 at the
2026-11-12 lock, which has `lfm3.cpp`; the input now buys a pin rather than a capability, and
`docs/follow-ups.md` owns whether to collapse it**). The card is driven by ./nvidia.nix — Pascal ⇒
`legacy_580`, `open = false`, and Vulkan rather than CUDA because CUDA 13's nvcc has no
Pascal target — the unit is ./llama.nix, and the front-door row is
`llama.int.leighhack.org` in `common/front-door.nix`: LAN ACL, no injected key, with the
note there saying why reusing `strata_api_key` was the wrong way to save adding a field.

Two facts about this endpoint are worth having at hand:

- **It is the first front-door name whose upstream is not on the wired block.** The door
  proxies `10.3.1.1 → 10.3.2.213:8080`, i.e. from VLAN 225 to VLAN 226, which works
  because the router routes both and its nftables allow it — and which changes character
  silently when the box moves to VLAN 225. The address in the row is read from
  `common/hosts.nix`, so the row follows; what does **not** follow automatically is any
  ACL anyone writes later on the assumption that `10.3.1.x` is where services live.
- **The weights are runtime data outside the store.** A switch cannot fetch them, an
  engine bump does not re-prepare them, and if the path is missing the unit restarts
  forever and the door answers 502. `journalctl -u llama-server` says `failed to load
  model`; that is the whole diagnosis.

### How the engine is built, and why it is not the input's own package

`llama-cpp-upstream` is the source; **nixpkgs' `llama-cpp` expression is the build**, and the
embedded web UI is built **with** it. Both halves are failure-driven. The argument for the
first (the `EACCES: copyfile` inside `webui-0.0.0.drv`, why `useWebUi` cannot be reached
through `.override`, and what each line of the `overrideAttrs` removes) is in the header of
./llama.nix — the short version is that upstream's flake makes `webui` an attribute of the
`mkDerivation` set, which makes it a *derivation input*, so asking for "no UI" does not stop
its **808 npm tarball derivations** from being built — measured at this pin with
`grep -c "^building .*\.tgz\.drv"` over the failing build's log, because the first version of
this sentence said "~830" off the top of an agent's head, which is the exact failure mode
`docs/nix-dev-workflow.md` warns about (a doc's "measured on the box" quoting somebody's
earlier guess). One of those 808 then fails.

nixpkgs' UI build works on this tree with **two pins**, each named by an error it printed and
each recorded in ./llama.nix: `npmDepsHash` is not nixpkgs' (that pin is for 0.6.0's
`tools/ui` lockfile — the right value is the `got:` the first fetch of *this* tree reported),
and `makeCacheWritable = true`, because npm 11 tries to `mkdir _cacache/tmp` inside the
read-only `npmDeps` store path even when offline.

### `https://llama.int.leighhack.org/` was a 404, and the two things that made it quiet

`{"error":{"message":"File Not Found"}}` under `Server: llama.cpp` is the engine, not the door.
The binary had **no embedded assets at all**, and the reason is a pair of defaults that conspire
to be invisible: `LLAMA_BUILD_UI` is **OFF** (`CMakeLists.txt:137`), and
`LLAMA_USE_PREBUILT_UI` is **ON** (:138), whose fallback is to *download* the built UI from a
Hugging Face bucket — impossible in the sandbox, and `scripts/ui-assets.cmake:144` then
generates an **empty** asset table **without failing the build**. So the engine compiled clean,
started clean, served `/health` and the API clean, and had no UI at the one path nobody had
asked for yet. Hence `-DLLAMA_USE_PREBUILT_UI=OFF` alongside `-DLLAMA_BUILD_UI=ON`: a build can
now be missing the UI loudly, never quietly. **The general shape — a default-ON network
fallback inside a build is a silent behaviour change the moment the sandbox denies the network —
is worth recognising on any package, not just this one.**

The second quiet thing: **the assets are stored gzipped inside the binary**, so `/` answers
`415 Error: gzip is not supported by this browser` to any client that sends no
`Accept-Encoding: gzip`. Every browser sends it; `curl` does not. So the check is
`curl -H 'Accept-Encoding: gzip' http://10.3.2.213:8080/ | head -c 80` — and an HTTP monitor
pointed at this endpoint either sends the header or probes `/health`, because a 415 on `/` is
not a fault. (And the UI is a SvelteKit app: its assets are
`/_app/immutable/bundle.*.js` and `/_app/immutable/assets/bundle.*.css`. A first reading of
those as `/assets/bundle.*.css` — 404 — was a `grep -oE '/assets/…'` matching the *tail* of the
real URL and then testing a path nothing requests: check the URL came out of the page, not out
of your pattern.)

Two commands re-verify the **build** claim above, because it is a claim about somebody else's
build:

```bash
nix build --no-link 'github:ggml-org/llama.cpp#packages.x86_64-linux.vulkan'   # fails in webui
nix eval --raw .#nixosConfigurations.services3.config.systemd.services.llama-server.serviceConfig.ExecStart
```

### Driven, measured (2026-11-11) — the LFM3 configuration this replaced

| | |
| --- | --- |
| Driver | `nvidia 580.178.04` (the `legacy_580` branch ./nvidia.nix pins — the last one with Pascal), `open = false`, 11,264 MiB total |
| Vulkan | `vulkaninfo --summary` → deviceName `NVIDIA GeForce GTX 1080 Ti`, driverName `NVIDIA`, Vulkan 1.4.312 — **device 0**, which is what `--device Vulkan0` in ./llama.nix names. Device 1 is `llvmpipe (LLVM 21.1.8, 256 bits)`: a working llvmpipe is normal here and useless for this, so read the *name*, not the count |
| VRAM as configured — `--ctx-size 262144`, q8_0 KV, `--n-gpu-layers all` | **3,026 MiB of 11,264 MiB** (27%; `nvidia-smi --query-compute-apps` shows llama-server holding 3,015 MiB). Weights are 1,050 MiB, so ~1,900 MiB is KV. The measured ladder from 32 KiB to 393 KiB — ~1,137 MiB fixed + **7.4 KiB/token** — is in ./llama.nix; it is what shows the ceiling is the model, not the card |
| Context ceiling | **262,144 = the GGUF's trained `ctx_train`**, which llama.cpp reports as `ctx_train (262144)` the moment you ask for more. 393,216 runs (3,971 MiB) but is rope extrapolation on an unmeasured model, so it is not configured; the lever for going past 262 KiB is `--rope-scaling yarn --yarn-orig-ctx 262144` **plus a retrieval re-measure** |
| Long-context retrieval | proven, not assumed: a 502 KB request through the door, **117,839 prompt tokens**, needle placed ~105k tokens in → returned correctly (`KESTREL-9-THISTLE`). Prompt evaluation at that depth ran ~680 tok/s; at 18k depth it was **1,349 tok/s** — a long prompt is seconds-to-minutes of prefill on Pascal, which is why the door row's `timeouts` are 3600s |
| Serving | `/` → the embedded llama.cpp UI (`<!doctype html>`…), with `/_app/immutable/bundle.CSvj5_dP.js` 2,884,479 b, `/_app/immutable/assets/bundle.T5LmKmrW.css` 304,601 b, `manifest.webmanifest`, icons — **200 direct and through the door**; `--ui`/`--no-ui` is the runtime switch and defaults to enabled |
| Decode | **105.6 tok/s** at short context, **84.0 tok/s** with ~18k tokens already in the KV (`/completion`, `predicted_per_second`). Decode is where the long context actually costs, and it costs about a fifth |
| Answering | `curl http://10.3.2.213:8080/v1/chat/completions` returns a `reasoning_content` body — the model spends the first tokens thinking, so `content` is empty at `max_tokens: 12`. That is the model, not a broken endpoint |

### Model changeover 2026-11-12: LFM3-1.7B → **Ternary-Bonsai-2-9B-Q2_0** (+ its `mmproj`)

The question that chose the new one was *what is the most advanced model this card can
actually serve*, asked of the card rather than of a leaderboard: every number below was
measured on this box with the shipped flags, and the candidates were bounded first by the
engine's own quant table and the card's 11,017 MiB of usable memory, not by taste.  What the
engine says of the served file on the live unit: `/v1/models` answers `n_params 8,953,803,264`,
`ftype Q2_0`, `size 2,545,600,512`, `n_ctx 262144 = n_ctx_train`, `n_embd 4096`,
`capabilities ["completion","multimodal"]`; the loader's banner says `general.architecture = qwen35`,
`n_layer 32`, `full_attention_interval 4`, `ssm_d_inner 4096` — a hybrid/SSM model, which is the
shape of every claim below.

Weights: **`Ternary-Bonsai-2-9B-Q2_0.gguf` 2,556,640,128 B** and its
**`Ternary-Bonsai-2-9B-mmproj-Q8_0.gguf` 624,229,664 B**, fetched 2026-11-12 out of
`prism-ml/Ternary-Bonsai-2-9B-gguf` into `/srv/models` — a directory the flake creates
(`systemd.tmpfiles.rules` in `./llama.nix`, a `d` rule that owns a directory without emptying
it) and nobody else populates: no switch downloads 2.4 GB of runtime weights, and the fetch
that did happen was a `systemd-run` unit (a 4m42s curl outlives a tool call) that needed
`--setenv=PATH=/run/current-system/sw/bin`, without which its first command is
`curl: command not found` and its log is the only witness.  LFM3's file (1,099,627,808 B) is
still at `/home/leigh-admin/Models/` and serves nothing; deleting it is a decision, not a
side-effect.

Measured on this card, whole card to itself — `systemctl stop llama-server` first:

| | measured |
| --- | ---: |
| offloaded | `33/33 layers` to Vulkan0 — 2,154.86 MiB of weights there, 272.81 MiB CPU-mapped |
| KV at 262,144 cells | **4,352.00 MiB** (q8_0 K 2,176 + V 2,176 over the 8 full-attention layers of 32 → **16.6 KiB/token**) |
| recurrent state | **50.25 MiB per slot** (32 layers: S 48.00 + R 2.25) — `n_slots = 4`, `kv_unified = 'true'` |
| compute buffers | 360.02 MiB on Vulkan0, 288.02 on `Vulkan_Host` |
| **peak VRAM, shipped flags including `mmproj`** | **8,005 MiB of 11,264 MiB** — 3.26 GiB free |
| decode | **63.1–64.0 tok/s** (`/v1/chat/completions` `predicted_per_second` 63.11; cli `-n 128`/`-n 256` 64.0, `graphs reused = 22`) |
| prefill | **305.6 tok/s** over a 1,650-token prompt (cli `print_timing`); 287.8 tok/s at 87 tokens, cold |

LFM3's decode, read off the journal of the configuration that was *still running* that
afternoon, is **60.1–66.3 tok/s** (`print_timing` lines: 60.08 and 66.34) — the table above's
105.6/84.0 was a 2026-11-11 measurement at a different depth.  So this swap bought a **5×
bigger model at the same tokens per second**, which is only possible because both models are
hybrid: hybrid models on this card move at ~100 GB/s *effective* (LFM3 measured 111 GB/s; the
dense Qwen2.5-VL at 290 GB/s), so an SSM layer costs about the same per token whatever its
width, and the per-parameter bandwidth penalty a dense 9B would pay — 2.6× at this size — never
appears here.  Do not carry the dense figure into a decision about a hybrid model.

**The 27B of the same family was fetched and measured too, because it is the boundary.**
`Ternary-Bonsai-2-27B-Q2_0` (**7,626,008,928 B**) + its 629,246,976 B `mmproj`:

| | measured |
| --- | ---: |
| offloaded | `65/65 layers` — **6,921.11 MiB** of weights on Vulkan0 (341.02 MiB CPU-mapped) |
| KV | 16 full-attention layers of the 64 → **33.0 KiB/token** (65,536 cells = 2,176 MiB) |
| recurrent state | **149.62 MiB per slot** (S 144.00, R 5.62) — four slots is 598 MiB of state |
| `-c 131072` | **fails**: `ggml_vulkan: Device memory allocation of size 570425344 failed` → `llama_init_from_model: failed to initialize the context: failed to allocate buffer for kv cache` |
| `-c 65536` + `mmproj`, 4 slots, shipped flags | starts at **10,788 MiB of 11,264 MiB** — 476 MiB of headroom |
| decode | **24.8–26.7 tok/s** (cli, `-n 128`, cold graphs included) |

So shipping the 27B is `modelName`/`mmprojName` + `ctxSize = 65536`, and the trade is 3× the
parameters for **2.4× slower decode, 4× less context, and 476 MiB of headroom instead of 3.26
GiB** — one Vulkan graph reallocation or one driver update takes it over the edge.  That is why
the 9B ships and the 27B waits on the shelf; and what "the better model" would mean here is an
eval, which nobody has run on the text side, so the 27B is a documented option and not a
recommendation.

**What cannot be considered at all, and why:** every Bonsai-2 size above 27B, and the dense
Qwen3.5 quants (`Q3_K_S` 11.45 GiB, `IQ4_XS` 13.95 GiB), do not fit the 11,017 MiB of usable
card at all — not "tightly", not at all.  And CPU spill is not a fallback on this box: four
Skylake cores decoding a 12 GB model spend their time on single-digit tok/s, which is a chat
timeout rather than a slow answer.

**Two refusals that look like the filename you typed.** prism-ml publishes each size in a
pruned/ternary build (`-PTQ1_0`, `-PQ2_0-MTP-Q8_0`) and a plain one (`-Q2_0`); **only the plain
one loads at this pin**, the others are refused at load.  `Ternary-Bonsai-2-9B-DFlash2-Q8_0`
(1,574,233,856 B, sitting on the shelf beside the two that loaded) is refused the same way:
`--spec-draft-model` answers `failed to load draft model`, and its header says
`general.architecture = dflash`, an arch 0.9.0 does not have.  So **no spec-decoding flag is
shipped**, and `--spec-type ngram-simple`, measured instead, bought nothing (62.8 t/s against
64.0 plain).  Ask the loader, not the filename: its banner prints `ftype : Q2_0`, and 2,545,600,512
bytes over 8,953,803,264 parameters is 2.27 bits/weight, i.e. this `Q2_0` *is* carrying ternary
blocks — what it does not carry is the pruned geometry the other builds trade for that name.

**`--flash-attn on` and `--cache-type-v q8_0` are one decision, not two.** With
`--flash-attn off` the engine will not initialise at all: `llama_init_from_model: quantized V
cache requires flash_attn to be enabled` (measured 2026-11-12, on the shipped flag set with one
flag removed).  And `on` is honoured only halfway, in the two wordings this box prints: the
cli's `resolve_fused_ops` line says `layer N is assigned to device Vulkan0 but Flash Attention
over K/V rows is assigned to device CPU (usually due to missing support)`, and the live server
answers the shorter `resolve_fused_ops: Flash Attention over K/V rows not supported, set to
disabled` — the same class of line the Qwen2.5-VL run produced on this box.  Either way the
flag is accepted and the K/V reduction is not a Vulkan graph, so flash attention is not what
makes this model fast on this card.  Neither sentence is a fault; both are why the two flags
move together.

**Raw `/completion` on a reasoning model answers 1–8 tokens and stops with `stop_type: "eos"`.**
Both models did it today on the bare prompt `"Q: … A:"`; the decode figures above come from
`/v1/chat/completions` or from the cli's `-p`, which is where `--reasoning-preserve` and the chat
template do their job.  A probe that reads a tiny `predicted_n` out of `/completion` is not a
broken endpoint.

This change moved no name, no port, no vhost and no ACL — `llama.int.leighhack.org` is the same
row in `common/front-door.nix` (its comment says what is behind it now), so `just check`'s
drift guards see nothing; what re-deploys is the unit's `--model`, `--mmproj`, `--alias` and the
directory that holds them. After the switch that first installed the driver, `nvidia-smi` still failed and
`lsmod` still showed `nouveau`: a `switch` cannot unbind a driver the running kernel has
already bound, so the new generation's `nvidia.ko` sat unused in
`/run/current-system/kernel-modules` while llama.cpp opened **Mesa's NVK** Vulkan device over
`nouveau` and started faulting it — `nouveau 0000:01:00.0: gr: TRAP ch 3 [02bf586000
llama-server[1710]]`, over and over. So the order on a box with no console is: read the *new*
generation's module tree (`find $OUT/kernel-modules/lib/modules/$(uname -r) -name
'nvidia*.ko*'`) and the nouveau blacklist in `$OUT/etc/modprobe.d/`; confirm the driver is
nowhere in the initrd (`grep -n initrd nixos/modules/hardware/video/nvidia.nix` — it is not,
which is what made rebooting an unattended bench box safe); **stop `llama-server` first**;
reboot; then `readlink -f /run/current-system /nix/var/nix/profiles/system-good`. And
`sudo modprobe nvidia` as a shortcut answers "why isn't it loaded" with a lie — `Module nvidia
not found in directory /run/booted-system/…` is about the *booted* (old) tree, not the new
generation's.

### Re-measuring the KV ladder

The table's VRAM row is a property of the KV quantisation, `--flash-attn on` and the engine, not
of the box, so it is worth re-deriving after any of those three move. This is the probe that
produced it — it stops the service, walks `--ctx-size` upward, samples the peak, and puts the
service back whatever happens:

```bash
BIN=$(systemctl show llama-server -p MainPID --value); BIN=$(sudo readlink /proc/$BIN/exe)
M=$(ls /srv/models/*-Q2_0.gguf)   # the served file; the ladder below was produced with
                                  # M=/home/leigh-admin/Models/LFM3-1.7B-Q4_K_M.gguf
sudo systemctl stop llama-server
for c in 131072 262144 393216; do
  $BIN --model $M --device Vulkan0 --n-gpu-layers all --ctx-size $c \
       --cache-type-k q8_0 --cache-type-v q8_0 --flash-attn on \
       --host 127.0.0.1 --port 8099 >/tmp/probe-$c.log 2>&1 &
  pid=$!; peak=0
  for i in $(seq 1 20); do
    sleep 3; m=$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits | head -1)
    [ "${m:-0}" -gt "$peak" ] && peak=$m
    kill -0 $pid 2>/dev/null || break
    grep -q "listening on" /tmp/probe-$c.log && [ $i -gt 6 ] && break
  done
  echo "ctx=$c peak=${peak}MiB $(kill -0 $pid 2>/dev/null && echo LISTENING || echo EXITED)"
  kill $pid 2>/dev/null; wait $pid 2>/dev/null
done
sudo systemctl start llama-server
```

Two things it will teach you if you run it blind: the model complains about `ctx_train` the
moment you pass its trained length (grep the probe log for `ctx_train`), and a `LISTENING` row
proves allocation, not quality — beyond 262,144 the rows are *numbers about VRAM*, and whether
the model still reads its own context is a different measurement, which is why the row above
proves retrieval with a needle instead of trusting the flag.

## Deploying it

`just deploy services3` from any box that has the commit — it resolves this box's address
out of `common/hosts.nix`, sshs as `leigh-admin` with the hop key, fast-forwards the
target's own checkout, and runs `nixos-rebuild switch --flake .#services3 &&
sudo nixos-confirm` **there** (rule 27: typed on services3 it degenerates to a local
switch, which is what you want on a Wi-Fi box).

Bootstrap facts, because the box was not always in the flake:

- Its checkout must live at `~/Projects/infrastructure-nix-flake` — the `deploy` recipe
  `cd`s to that path and has no other. **Its remotes were the anomaly, not its keys:**
  until 2026-11-15 this box's *only* remote was services1's checkout — a leftover from
  the other fleet it was assimilated from — so `git pull` here meant something else
  entirely, and printed `(leigh-admin@10.3.1.20) Password:` rather than an error. Both
  halves are now written down. The remotes match the rest of the fleet (`origin` is
  GitHub like everywhere else, `infra` is services1's checkout over the hop — the same
  pair `machines/gw/README.md` item 3 names), and the prompt was ssh offering **no
  identity at all**: this box's `~/.ssh` held only `agent-hop-key` (mode `0600`, no
  `config`, no `known_hosts`), which is not one of the file names ssh tries on its own,
  and **no ssh-agent runs on these boxes** — so a `git` ssh that carries no `-i` has
  nothing to offer and walks through to `Password`, on a box whose sshd is
  `prohibit-password`, inside a job with no terminal to answer it. That half is now
  declarative in the **system** client config — `common/ssh-hop.nix`'s
  `programs.ssh.hosts`, rendered into `/etc/ssh/ssh_config` — on every flake machine.
  So this box updates itself with
  **`git pull infra`** and needs no `GIT_SSH_COMMAND` noise; that half closed on 2026-11-15
  and everything quoted above it is what was measured *then*. On 2026-11-22 the *other* half
  closed too: the box now holds a credential GitHub accepts, so a bare `git pull` here means
  GitHub and a bare `git push` here lands the commit. Read the two failure strings above as
  history rather than as the answer — and note that they are still exactly what `gw` and
  `services2` print, because neither has a credential of its own. The box's own half is the
  section below, "Getting commits on and off this box", which says what is runtime state on
  the box and what is flake config, and what is still owed. A `ahead N` in `git status -sb`
  there **was** a stale `origin/main` ref; after 2026-11-22 it is a live comparison, because
  `main` now tracks `origin` like it does on every other box in the fleet.
  `just deploy services3` from a box that holds the commit remains the way that both
  fetches *and* switches, and it is the one that keeps the GOLDEN RULE's confirm in the
  same shell invocation.
- **The shared age key must be installed before the first switch**, or activation fails
  when sops-nix decrypts the two secrets `common/sops.nix` declares for every box:
  `just install-sops-key` on this box, with `SOPS_KEY_FILE` pointing at a copy of the
  fleet key (`/var/lib/sops-nix/key.txt` on services1 — copy it root-to-root, never
  through a terminal, AGENTS.md rule 13). The `z` tmpfiles rules in `common/sops.nix`
  are a *repair*, not an install.
- `leigh-admin`'s private hop key is copied by hand, per machine
  (`~/.ssh/agent-hop-key`, mode 0600, owner `leigh-admin`), and the public half is
  authorized everywhere by `common/users.nix`. Without the private half the box evaluates
  and switches but cannot delegate its builds to aibox, and the failure is root's ssh
  with nothing in the log but `Host key verification failed` — see rule 25 and
  `docs/nix-dev-workflow.md` ("Remote building: the hop is not the operator's ssh"). The
  host key side is already pinned in `common/ssh-known-hosts.nix` by `just known-hosts`.

## How it was actually switched, and the four things that cost time

The box was onboarded on 2026-11-11 from `services1`, over the one Wi-Fi path it has,
and the activation was made **deterministic** rather than optimistic: the toplevel was
built here (`nix build .#nixosConfigurations.services3.config.system.build.toplevel`),
its closure diffed against what the box already had (`nix-store -qR` on both sides,
1,142 paths / ~4.4 GiB), streamed in as root
(`nix-store --export … | zstd | ssh '… | sudo -n nix-store --import'`, because the
account there is not a trusted user), and hash-verified on arrival
(`sudo nix store verify --recursive <toplevel>`). Two gates ran before the switch, both
because losing this box means walking to it: the new kernel's module tree was checked to
contain `rtw88_8821cu.ko.xz` **and** `rtw88/rtw8821c_fw.bin` (the dongle's driver and
firmware — a kernel without them is a box with no network), and
`nixos-rebuild dry-activate` was read in full — which is what found the `would remove
user ‘cjdell’` line that ./users.nix exists to answer.

Four things did not go the way they look like they should. All of them are about
bootstrapping a machine that is not yet in the flake:

1. **A `systemd-run` unit has no `/run/current-system/sw/bin` on its PATH.** The switch
   was detached on purpose (activation restarts `sshd` and `wpa_supplicant`, so it must
   not die with the ssh session that started it) — and then `nixos-rebuild: command not
   found`, `nix-env: command not found`, and finally `nixos-confirm: readlink: not
   found` were the only failures. Absolute paths (`/run/current-system/sw/bin/…`) or a
   `PATH=` line in the script. The last one mattered most: the activation succeeded and
   the confirm did not, which left the five-minute countdown running on a box reached
   over the link the countdown might cut.
2. **`git+file://$PWD?dirty=1` cannot read a checkout owned by another user, and root's
   `safe.directory` does not fix it inside a unit.** The nix client that resolves the
   flake refuses with `repository path … is not owned by current user (libgit2 error
   code = 7)`. From a `sudo` shell this is why `sudo git config --global --add
   safe.directory <checkout>` is already present on services1 and services2 (both
   carry it) — but a `systemd-run` unit does not run with `HOME=/root`, so it reads no
   such file. On this box the checkout was `cjdell`'s, so the first switch could not be
   a flake switch at all: it was `nix-env --set <toplevel>` +
   `<toplevel>/bin/switch-to-configuration switch`, i.e. the "guarantee the exact
   toplevel you built is the one that runs" path, which needs no evaluation and no git.
   `sudo git config --global --add safe.directory` was then set for both checkouts, and
   `/home/leigh-admin/Projects/infrastructure-nix-flake` was cloned (from services1 over
   the hop key — the same peer-fetch `just deploy` falls back to, and it works here) so
   the *next* deploy of this box is an ordinary one.
3. **Installing the new fleet's age key breaks the old config's activation.** The infra
   key went in before the switch (it has to be there, or `setupSecrets` fails); the old
   config still had its own `sops.secrets` pointing at its own `secrets.yaml`, so a
   re-activation of the *old* system afterwards failed with `Error getting data key: 0
   successful groups required, got 0`. That is the sound of the wrong config activating,
   and it was nearly the right answer to the wrong question — see the next item.
4. **`/nix/var/nix/profiles/system/bin/switch-to-configuration` is the old system's
   script.** If the profile did not advance (item 1), running that path re-activates the
   generation that was already running — which looks like a failure of the new config
   and is actually a failure of the old one, for a reason the new config has nothing to
   do with (item 3). `readlink -f /nix/var/nix/profiles/system` **before** activating is
   the check; `switch-to-configuration` belongs to the toplevel path, not the profile.

Nothing here was rolled back: `auto-rollback` armed once, was confirmed
(`readlink -f /run/current-system /nix/var/nix/profiles/system-good` → equal), and the box kept
its address and its Wi-Fi association throughout. **The one reboot** came later and for its own
reason — binding the NVIDIA driver, above — and came back on the same generation.

5. **`nix copy`'s ssh is root's ssh, and it needs an identity named somewhere.** The 4.4 GiB
   graft streamed fine, and then `nix copy` of the new toplevel died instantly with `failed to
   start SSH connection to '10.3.2.213'` — for two separate reasons, both root-only. First
   `Host key verification failed`: services1's `/etc/ssh/ssh_known_hosts` is rendered from
   `common/ssh-known-hosts.nix`, and that file **was** correct — the stale `.213` line was in
   `/root/.ssh/known_hosts`, which ssh reads *first*, and it predated this box entirely. That
   is rule 25's "a hand-written root `known_hosts` on one box is what this replaces", arriving
   exactly as described, and `sudo ssh-keygen -f /root/.ssh/known_hosts -R 10.3.2.213` is the
   repair. Second, once verified, `Too many authentication failures`: the daemon's ssh offers
   whatever root's config names, and `/root/.ssh/config` had an identity stanza for aibox and
   nobody else. Adding the matching `Host 10.3.2.213` stanza made the copy work — and the
   escape hatch worth knowing without touching that file is **`sudo env NIX_SSHOPTS='-i …
   -o IdentitiesOnly=yes' nix copy …`**, because the URL form `?ssh-option=…` is not a thing
   (`warning: unknown setting 'ssh-option'`). The identity half of the **operator** account is
   declarative now (`common/ssh-hop.nix`, `programs.ssh.hosts` — see the Bootstrap bullet and
   `docs/nix-dev-workflow.md`), so `git pull infra` needs no inline `-i` on a switched box.
   Root's half is still hand-written and is an
   open item below.

**Both boxes are now on a commit.** `services3` and `gw` were activated from an *uncommitted*
working tree for most of 2026-11-11 — the state rule 20 turns into a day of confusion, and the
reason this change is committed rather than left staged. `just deploy services3` / `just deploy
gw` therefore reproduce what is running. Re-verify with `git -1` on each box (rule 20), and
remember `gw` is the box that has previously run *ahead* of its own line —
`machines/gw/README.md` owns that recipe.

## Getting commits on and off this box, on its own (2026-11-22)

Until that date this box could *fetch* — `git pull infra` is services1's checkout over the
LAN hop — but it could not put a commit on GitHub: it held no credential GitHub accepts,
so anything committed at the bench had to be relayed through services1. It now has its own
credential and a bare `git pull` / `git push` here mean GitHub, which is what `just deploy
services3`'s first branch (`git fetch -q origin`) has always assumed was possible: since
2026-11-22 it answers, from any box, instead of falling through to the peer fetch.

Four facts, in the order that keeps each one's failure mode a different string:

1. **The private key is the box's own and was made on the box** — `ssh-keygen -t ed25519
   -N '' -f ~/.ssh/id_ed25519 -C services3@leighhack.org`, fingerprint
   `MD5:80:c7:03:86:d9:b1:ad:89:a1:25:b7:5e:9b:a0:26:8c` (`ssh-keygen -l -E md5 -f
   ~/.ssh/id_ed25519` re-reads it). The name `id_ed25519` is the whole reason the box needs
   no ssh config of its own: it is one of the file names ssh tries when nothing is asked for,
   and `git`'s ssh asks for nothing. It cannot be declarative: this pinned nixpkgs writes no
   home files (`common/users.nix` names the refusal), and `common/ssh-hop.nix`'s identity
   block is keyed to **`common/hosts.nix`'s addresses on purpose**, so it does not reach
   github.com — a `Host` block that put the *LAN* hop key on the GitHub remote is the
   documented leak trap (`Permission denied (publickey)`), and a generated file cannot do it
   by accident because the host is what the block matches on.
2. **What GitHub was given is a repo-scoped *deploy key* with write access, not a key
   registered on the account** — and that is a decision, not a habit. `ssh -T git@github.com`
   on services1 and aibox answers `Hi cjdell!`, i.e. their `id_ed25519` are *account* keys,
   which reach every repository that account can — including the thingino firmware repo whose
   history carries the cameras' Wi-Fi PMKs (rule 30, `docs/camera-thingino-build-deploy-2026-10-31.md`
   §7). A key on a bench box at the edge of a Wi-Fi dongle should not be able to read that.
   A deploy key pushes and pulls *this* repository and nothing else, so it is the weaker grant
   with the same effect. It was made with
   `gh api -X POST repos/leigh-hackspace/infrastructure-nix-flake/keys -f read_only=false -f title=…`
   run from services1 — the only box with `gh` configured, and its token is on that box's
   `gh`, in no store path and no flake field (rule 13). The grant's *state* is readable only
   there — `gh api repos/leigh-hackspace/infrastructure-nix-flake/keys --jq '.[]|{title,read_only,verified,created_at}'`
   answers exactly one key, `read_only: false`, `verified: true`, created `2026-11-22T14:29Z`
   (GitHub's own record is the date; the title it shows is a cosmetic label for the box and the
   day, not an identity). Two API facts worth knowing before trying to tidy that label: **a
   deploy key cannot be added twice** — the same key material POSTed again is refused with
   `key is already in use` (422), so a rename is a *delete then recreate*, which is the one
   window in which this box cannot push at all — and **`PATCH` on the key is refused with 404**
   by services1's token (scopes `gist, read:org, repo`), so renaming it there is a web-UI
   decision rather than an API edit. **`read_only=false`
   is load-bearing**: a read-only deploy key authenticates a `git push` connection and then
   refuses the write, which reads as a broken remote rather than as the wrong grant.
3. **Proven in both directions, and back to nothing left behind**, read off the box on
   2026-11-22 with `SSH_AUTH_SOCK` unset (docs/nix-dev-workflow.md: a dead agent socket makes
   a hop *hang*, silently):

   ```bash
   git ls-remote origin HEAD          # edde14e6… — the handshake, and github.com's host key accepted
   git fetch origin                   # fetched
   git push origin HEAD:refs/heads/key-check-services3        # PUSHED — the write grant, exercised
   git push origin --delete key-check-services3               # DELETED
   git ls-remote origin refs/heads/key-check-services3        # empty: residue=0
   ```

   The write proof is a scratch branch pushed and deleted in one invocation, and not a push of
   something GitHub already has: pushing a commit that already exists is a no-op that never
   asks the write grant, so "it authenticated" and "it can push" would not have been
   distinguished. The scratch branch is the cheapest thing that makes them the same failure.
4. **The host half of that hop is flake config now, and this box is not yet switched onto it.**
   `common/ssh-known-hosts.nix` grew github.com's three published keys on 2026-11-22, and the
   fleet deploy of that day is what carries them to this box — they ride the same flake source
   as everything else in the commit, and nothing engine-shaped rode with them (`./strata.nix`'s
   args did not move). Before that deploy the box's github.com host keys came from its own
   `~/.ssh/known_hosts` — a hand-made half, the shape a reinstall loses silently, and the
   reason the table entry is not optional. Re-verify with
   `grep -c github /etc/ssh/ssh_known_hosts` on this box: it goes from `0` to `3`.

Two leftovers the assimilation left were fixed in the same breath, both of which lie to `git
status` rather than to ssh:

* `origin/HEAD` pointed at **`refs/remotes/infra/main`**, which is not a remote branch — it is
  what `git remote set-head origin --auto` fixed, and it is a fossil of the 2026-11-11 state
  where this box's only remote *was* the peer under the name `origin`.
* `main` tracked `infra/main`. It now tracks `origin/main` (`branch.main.remote=origin`,
  `branch.main.merge=refs/heads/main`), which is what makes a bare `git pull`/`git push` mean
  GitHub here and makes `git status -sb`'s `ahead N` a live fact instead of the stale-ref trap
  the runbook warns about.

The pull that ran on 2026-11-22 was `--ff-only` from `0bb5495` to `edde14e` on a clean tree
(`dirty=0`, and `0bb5495` proved an ancestor of `origin/main` first), so **the checkout is now
ahead of what is deployed on this box**: `git log -1` here is no longer the generation that
runs (rule 20), and the next `just deploy services3` is what carries it — the commits it
carries are **docs and one new ssh-host-keys row**; the engine's args did not move, so this
switch does not restart strata.

Re-verify, in this order (the first three are the box, the last is services1's, since `gh`
lives there and the grant is off-box state this flake cannot see):

```bash
# on services3 — `just repo-status` from anywhere prints the same answer as origin=reaches
cd ~/Projects/infrastructure-nix-flake && git status -sb | head -1   # ## main...origin/main
git ls-remote origin HEAD                                            # a rev, not an error
git config branch.main.remote                                        # origin
ssh-keygen -l -E md5 -f ~/.ssh/id_ed25519                            # the fingerprint above
# on services1 (the only box with gh configured)
gh api repos/leigh-hackspace/infrastructure-nix-flake/keys --jq '.[]|{title,read_only}'
```

## Open items

| | |
| --- | --- |
| Endpoint auth | `strata3.int.leighhack.org` is **LAN-only**, and `strataBearer = true` injects `strata_api_key` like the other two Strata rows (one engine credential, one door header — a per-engine key would 401 the door's own path). A public name would take `sso = true` + `acl = "none"` in the same change, like `strata.leighhack.org`. |
| Privileges | `strata.service` runs as root (as both other Strata engines do); `.nix`'s system account is still open in `docs/follow-ups.md`. The weights are not under `/home/leigh-admin` (mode `0700`): `./llama.nix` creates `/srv/models` (`0755`, root) by `tmpfiles` rule and reads the model there. What is left of that upgrade is the **system account** — the unit is still uid 0, still inherits root's `HOME` unless `StateDirectory` says otherwise, and both Strata engines remain in the first half. The home's mode was the actual blocker, so this half of the change is the half that mattered. |
| ~~Context headroom~~ | **Closed 2026-11-12 at 262,144** — the model's trained `ctx_train`, at 3,026 MiB of 11,264. What is left as a *decision*, not a gap: going past the trained length with yarn (./llama.nix has the ladder and the reasoning), which needs a retrieval measurement, not a bigger number. |
| Power | **Still open, and now rackable.** It was left out of `machines/gw/ups.nix`'s `shutdownTargets` because nobody had checked what it is plugged into, not because leaving it out was decided — and on 2026-11-25 it became a wired member of the services block beside services1 and services2, so it belongs in that list and in `crates/ups-watchdog/src/plan.rs`'s order list (the order is `docs/ups-and-fleet-shutdown.md`'s argument). Not shipped with the VLAN move: an address move and a shutdown-policy change are separate decisions, and the second needs the UPS's measured 300 s grace re-read against a fourth server. |
| Wi-Fi PSK | **Closed 2026-11-25** — the box is racked and wired. The PSK is in `/etc/NetworkManager/system-connections/WiFi Go Brrr.nmconnection` (root, 0600), which this flake has never managed and still does not; `./vlan225.nix` forces NetworkManager off so the dongle cannot silently become a second DHCP client on a NIC networkd owns. The dongle's MAC left `common/dns-records.nix` in the move's commit, so nothing in the flake invents 10.3.2.213 again. Putting the box back on Wi-Fi is a decision with a named shape (`sops.secrets` + `networking.networkmanager.connections`, which renders to `/run/...` and so will not fight the `/etc` file), not a resurrected file. |
| `/` free space | Model data moved to the NVMe, so `/` only carries the store and the generations; measured 2026-11-21 after the engine's closure, `df -h /` has tens of GiB free and the old generations are still prunable (see above) if a nixpkgs bump needs the room. |
| Root's ssh identity | `nix copy`/remote-build ssh runs as root out of `HOME=/root`, and the identity it offers comes from a **hand-written `/root/.ssh/config`** (aibox stanza since Sep 2025, services3 added 2026-11-11 — item 5 above). The *host keys* are declarative (rule 25); the identity is not, so `nix copy` to a new box fails with `Too many authentication failures` until a stanza is added by hand or `common/ssh-hop.nix` grows an absolute-path identity for the copying side (remote *builds* are gone since 2026-11-23, so root's ssh is now only the copy path). **The operator account's half closed 2026-11-15:** `common/ssh-hop.nix` now carries `programs.ssh.hosts` for the network's own addresses (`IdentityFile ~/.ssh/agent-hop-key`, `IdentitiesOnly yes`, `ConnectTimeout 5`), rendered into `/etc/ssh/ssh_config` — the *system* file, because this nixpkgs has no home-file option in `users.users.<name>` at all (`just check` refuses it; `machines/gw/README.md`'s "authorized keys are a fact about the directory sshd reads" is the same shape). Root is **not** covered by that block: `~` in `IdentityFile` expands to `/root`, where no `agent-hop-key` exists, so root still needs its absolute-path stanza and `sudo env NIX_SSHOPTS='-i /home/leigh-admin/.ssh/agent-hop-key -o IdentitiesOnly=yes' nix copy …` remains the escape hatch. Re-verify the operator half with `ssh -G 10.3.1.20 | grep -E 'identitiesonly|agent-hop'` as `leigh-admin` (two lines once deployed; `ssh -G` also prints ssh's five built-in defaults on an unswitched box, so count them) and by the checkout's own `git remote -v` answering `origin` + `infra`. |
| Bad-RAM fence (WIP) | **Still a stash on this box, not a branch, and not forgotten.** The box's checkout is clean at the fleet commit, so the only trace of the bad-RAM work — `machines/services3/bad-ram-fence.nix` plus the measured `bad-ram-pages.txt` / `bad-ram-reservations.txt` and the README diagnosis — is `stash@{0}` on **this** box, whose own message carries the re-verify. Measured 2026-11-25, the conflict set on popping is **exactly three files** (`comm -12` of the stash's file list against `git diff --name-only 4c19476..<fleet-head>`): `docs/follow-ups.md`, `machines/services3/default.nix`, `machines/services3/README.md` — the stash's `justfile` edit does **not** conflict, because the 2026-11-25 commits moved a recipe into `just/checks.just` instead, and the three new files land cleanly. Re-verify before popping with those same two commands (a later fleet commit changes the answer), and pop only where the fence is still wanted: `git stash pop`, then read the rendered `machines/services3/default.nix` (rule 10 — a merge marker in a module the flake imports is a build error, not a warning). |
| Monitoring depth | Scraped **once services1 next switches**: the three scrape rows are in `machines/services1/services/observability.nix`, and a `dry-activate` of that change showed it would stop and restart six of services1's apps (`filestore`, `gocardless-dashboard`, `moonraker-exporter`, `prometheus`, `status-dashboard`, `switch-gui`) — the crates' unit paths move because the flake source moved. That is a services1 deploy decision (`docs/network-upgrade-runbook.md`), not a fee for adding a box, so it was not paid here. Until then the exporters run on this box and nobody reads them. Alerting is the same shape and now possible: `common/monitoring-endpoints.nix` is the gatus half, its guard wants an `https *.int` endpoint, and `strata3.int.leighhack.org` got its `/health` row in the same change as the door row — it goes live with the deferred services1 deploy, not as its own restart of six apps. |
| GitHub credential | **Closed 2026-11-22, both halves**: the box has its own `~/.ssh/id_ed25519` registered as a repo deploy key with write access (the section above carries the receipts, the grant's rationale, and both re-verify blocks), and pushes and pulls GitHub itself; and the `github.com` keys are now the row in `common/ssh-known-hosts.nix` that this box's switch of that day carries (re-verify `grep -c github /etc/ssh/ssh_known_hosts` = `3`; before it the count was `0` and the keys came from a hand-made `~/.ssh/known_hosts`, which a reinstall loses silently). |

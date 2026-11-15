# services3

The sixth configuration in this flake and the fourth deployable box. **On the bench, on
Wi-Fi, running nothing yet.** This file owns what that means: the measured hardware, the
two things that make it different from the other services boxes, the move to VLAN 225,
and what was deliberately left behind when the box came off its previous fleet.

Nothing here is a guess unless it says it is. Everything under "Measured" was read off
`10.3.2.213` on 2026-11-11 through the account the box arrived with; the previous owner's
write-up is quoted only where it is the *only* record (and then as quoted, not as fact).

## Measured

| | |
| --- | --- |
| Board / CPU | ASUS Z170 PRO GAMING, i7-6700 (Skylake 4c/8t, `kvm-intel`), 16 GiB RAM |
| GPU | **exactly one, and it is a GTX 1080 Ti (GP102/Pascal) on `01:00.0`** (+ its HDMI audio function `01:00.1`), driven by this flake on `nvidia 580.178.04` — ./nvidia.nix. Re-measured 2026-11-12 because "why is the 1060 missing?" was asked: `lspci -nn -d 10de:` lists those two functions and nothing else, NVML reports `GPU 0` only with `/dev/nvidia0` the sole card node, and `llama-server --list-devices` prints one line. **There is no second GPU in this box.** The network's GTX 1060 lived in **aibox** and was physically removed on 2026-09-02 — `docs/gtx1060-followup.md`, "STATUS: HARDWARE REMOVED", which is also where its (negative) llama.cpp verdicts are recorded |
| No iGPU either | `lspci -nn \| grep -ciE 'intel.*(vga\|display\|3d)'` = **0** — the i7-6700's HD Graphics 530 has no display function on this bus (disabled in the firmware), which is why `vulkaninfo` shows the NVIDIA ICD and Mesa's `llvmpipe` and nothing in between. An earlier version of this file and of ./llama.nix asserted the iGPU was a second Vulkan device; it had never been measured |
| Free slots | DMI slot table: `PCIEX16_1` **In Use**, `PCIEX16_2` **Available**, `PCIEX16_3` **Available** (both reported "Short" — the board's electrically-x4 slots), `PCIEX1_1` Available; and the kernel log has no BAR/MMIO refusal, so nothing is seated-and-refused. A second card could therefore be fitted. What that changes is `vulkanDevice` in ./llama.nix (the index order is ICD load order, **not** PCI order) and, if the intent is one model across two cards, the `--device` list — not the driver, and not this table's VRAM ladder |
| Disk | `sda` 111.8 GiB: `sda1` 1 GiB vfat (label `BOOT`) + `sda2` 110.8 GiB ext4 (label `ROOT`). 84 GiB used, **20 GiB free**, `/nix/store` = 77 GiB at arrival |
| Boot | UEFI 2.40, systemd-boot, Secure Boot off, no TPM. `mitigations=off` on the live command line |
| Swap | **none** |
| Wired NIC | `enp0s31f6`, Intel I219-V, MAC `70:8b:cd:a8:50:bc` — **no cable**, `NO-CARRIER` |
| Wi-Fi | `wlp0s20f0u1`, MAC `90:de:80:e1:4e:42`, Realtek RTL8821CU (`0bda:c811`) on `rtw88_8821cu`, connected to SSID `WiFi Go Brrr` |
| Address | `10.3.2.213/24`, gateway `10.3.2.1`, DNS `10.3.2.1`, search `int.leighhack.org` — i.e. **VLAN 226**, the wireless block, not the services block |
| IPv6 | two temporary SLAAC addresses in `2001:8b0:1d14:226::/64`; no stable address, hence `ipv6 = []` in `common/dns-records.nix` |
| Reachability | `10.3.1.1`, `10.3.1.20`, `10.3.1.32` and `cache.nixos.org` all answer a ping from the box, so builds can delegate to aibox and substitutes work |
| Host keys | `ed25519` + `rsa`, both dated Aug 2025 — the box's own, unchanged by this flake and pinned in `common/ssh-known-hosts.nix` |
| OS at arrival | NixOS 26.11 (Zokor), nixpkgs `26.11.20261017.09063bd`, kernel 6.18.57, nix 2.34.9 |

## What it runs

**Nothing, deliberately.** The module list (`default.nix`) is a box that has been
brought under management and measured but not yet given a job: `common/base.nix`, the
shared modules every services box gets through `flake.nix`'s `commonModules` (sops,
podman, crane args, build delegation to aibox — verified rather than assumed: `nix build
--max-jobs 0 --option substitute false nixpkgs#fastfetch` on the box compiles on aibox and
copies the result back, which proves the hop, the `trusted-users` half and the pinned host
keys together — the NAS client with no exports declared, the tool set, the users), plus
`common/exporters.nix` (`node`/`systemd`/`smartctl` on `:9100`/`:9558`/`:9633`, all three
listening). This box's three scrape rows are written into services1's Prometheus config
but **do not scrape it yet** — they land when services1 next switches, and until then
nothing reads those ports (`Monitoring depth` below has what that costs and why it was not
paid here).

The first real service added here gets its own module in this directory and a line in
`default.nix`'s import list — not a stanza in `configuration.nix`. If it needs the NAS,
it also declares `after` **and** `requires` `wait-for-nas.service` and keeps restarting
forever (AGENTS.md rule 12), and the export goes in this directory's
`hardware-configuration.nix` as `infra.nas.exports`, which is what `common/nas.nix`
generates both the mount and its automount unit from.

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

## The move to VLAN 225 (the rack, not the bench)

The box is here temporarily; the intent is a wired member of the services block. Order
matters because each step's failure mode is "unreachable", and the two tables live on
two different machines:

1. Cable the I219-V into a switch port that carries **VLAN 225 tagged** — a trunk,
   exactly like services1's and services2's ports. An access port gives `vlan225` no
   address. (`docs/procurve-switches.md` for the port config; `crates/switch-gui/` is
   the only thing that saves a change, and only when asked.)
2. Pick the address out of the wired block's fixed run and change the **three** facts in
   one commit: the row in `common/dns-records.nix` (name `services3`, `vlan = 225`, new
   `ipv4`, wired MAC, and — if the box is given a stable IPv6 — an `ipv6` it really
   owns, or keep `[]`), `common/hosts.nix`, and `machines/services3/vlan225.nix`'s
   header. `machines/gw/lib/check-network-tables.nix` is the guard that refuses a
   `common/hosts.nix` address no reservation answers; `just check-eval` runs it.
3. Deploy **gw** so the router actually hands out the new reservation
   (`just deploy gw`, GOLDEN RULE, then `systemctl start front-door-cert-check.service`
   as always on that box), and confirm the rendered line:
   `grep services3 /etc/dnsmasq.d/network.conf`.
4. Swap the module: in `machines/services3/default.nix`, `./networking.nix` out,
   `./vlan225.nix` in, and delete the parking header. Then switch **on the box** (the
   first wired switch cannot be made over the old link, so this is the one step that
   wants a hand at the bench or a working serial/`nixos-fail` path).
5. `just known-hosts` for nothing here (the host keys do not change), but the *dongle's*
   reservation should then be deleted from `common/dns-records.nix` in the same commit,
   so the table says one thing about a machine that has one address.

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
locally at `max-jobs = 1`). `common/build-farm.nix` replaces it with aibox.

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

## The engine: llama.cpp (Vulkan) serving Ternary-Bonsai-2-9B-Q2_0 (+ vision)

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
  `cd`s to that path and has no other. It has no GitHub credentials of its own, so
  `git fetch origin` fails there and the recipe's peer-fetch from `10.3.1.20` over the
  hop key is what updates it.
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
   (`warning: unknown setting 'ssh-option'`). The identity half is still hand-written and is an
   open item below.

**Both boxes are now on a commit.** `services3` and `gw` were activated from an *uncommitted*
working tree for most of 2026-11-11 — the state rule 20 turns into a day of confusion, and the
reason this change is committed rather than left staged. `just deploy services3` / `just deploy
gw` therefore reproduce what is running. Re-verify with `git -1` on each box (rule 20), and
remember `gw` is the box that has previously run *ahead* of its own line —
`machines/gw/README.md` owns that recipe.

## Open items

| | |
| --- | --- |
| Endpoint auth | `llama.int.leighhack.org` is **LAN-only and keyless** — the LAN ACL is the gate, and the model is a public weight. A public name takes `sso = true` + `acl = "none"` **and a key of its own** in the same change; `strataBearer` is not it, being `strata_api_key` and another engine's credential. |
| Privileges | `llama-server` still runs as root, but since 2026-11-12 the weights are no longer under `/home/leigh-admin` (mode `0700`): `./llama.nix` creates `/srv/models` (`0755`, root) by `tmpfiles` rule and reads the model there. What is left of that upgrade is the **system account** — the unit is still uid 0, still inherits root's `HOME` unless `StateDirectory` says otherwise, and both Strata engines remain in the first half. The home's mode was the actual blocker, so this half of the change is the half that mattered. |
| ~~Context headroom~~ | **Closed 2026-11-12 at 262,144** — the model's trained `ctx_train`, at 3,026 MiB of 11,264. What is left as a *decision*, not a gap: going past the trained length with yarn (./llama.nix has the ladder and the reasoning), which needs a retrieval measurement, not a bigger number. |
| Power | Not in `machines/gw/ups.nix`'s `shutdownTargets` (see above), and `crates/ups-watchdog/src/plan.rs`'s order list is the same three. |
| Wi-Fi PSK | Off-flake (see above). Decide when it is racked: delete it with the dongle's reservation, or make it declarative. |
| 20 GiB free | Prune the old generations (see above) or add disk before giving the box a build. Measured **9.7 GiB free** with the engine and driver installed. |
| Root's ssh identity | `nix copy`/remote-build ssh runs as root out of `HOME=/root`, and the identity it offers comes from a **hand-written `/root/.ssh/config`** (aibox stanza since Sep 2025, services3 added 2026-11-11 — item 5 above). The *host keys* are declarative (rule 25); the identity is not, so `nix copy` to a new box fails with `Too many authentication failures` until a stanza is added by hand or `common/build-farm.nix` grows an identity for the delegating side. |
| Monitoring depth | Scraped **once services1 next switches**: the three scrape rows are in `machines/services1/services/observability.nix`, and a `dry-activate` of that change showed it would stop and restart six of services1's apps (`filestore`, `gocardless-dashboard`, `moonraker-exporter`, `prometheus`, `status-dashboard`, `switch-gui`) — the crates' unit paths move because the flake source moved. That is a services1 deploy decision (`docs/network-upgrade-runbook.md`), not a fee for adding a box, so it was not paid here. Until then the exporters run on this box and nobody reads them. Alerting is the same shape and now possible: `common/monitoring-endpoints.nix` is the gatus half, its guard wants an `https *.int` endpoint, and `llama.int.leighhack.org` is one since the door row landed — so `/health` can be added to that table, on the same deferred services1 deploy rather than as its own restart of six apps. |

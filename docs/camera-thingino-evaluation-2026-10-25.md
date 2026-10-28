# thingino, measured against this fleet: it is Ingenic-only, and our cameras are not Ingenic

Asked on 2026-10-25: *should the cameras run **thingino** instead of OpenIPC?*

**Answer: no, and not "not yet" — the question does not type-check against this hardware.**
thingino is an open-source firmware for **Ingenic SoC** IP cameras; the three OpenIPC cameras
in this fleet self-report `ssc337 (infinity6b0 family)` (cam5, cam9) and
`ssc333 (infinity6b0 family)` (cam7), which are **SigmaStar** parts, and the fourth device on
that VLAN (cam1) is neither (measured below). thingino ships no image for any SigmaStar chip,
its build system cannot currently produce one, and the single SigmaStar SoC name in its
Kconfig (`ssc30kq`, family `infinity6e`) is not one of ours. The path forward for these three
boxes stays the OpenIPC update that is already staged and pre-flighted
(**`./camera-sd-card-firmware-flash.md`**), 2 years behind as it is.

What this doc adds is the measurement (the repo's only prior statement on thingino was
`cameras/c110.md` §8.6, marked ⚠️ guess), the two things thingino *is* worth to us — one for
the next camera purchase, one as reading for the faults we already log — and the exact
commands that re-answer the question in a year, because upstream moved in August 2026 and the
flat sentence "thingino is Ingenic-only" will need re-reading when it moves again.

Every claim below is followed by the command that produced it. Everything was run on
2026-10-25 from services1 against `themactep/thingino-firmware` at `f083382` (master,
2026-10-23, "motors: leave SPI axis inversion to the daemon") over the public GitHub API and a
`--depth 1 --no-recurse-submodules` clone in `/tmp`, plus unauthenticated probes of
10.3.2.6. No camera was written to; no credential was read.

---

## 1. What thingino is, in the terms this repo uses

| | |
| --- | --- |
| Project | `themactep/thingino-firmware` (MIT, created 2022-06-08, 2,275 stars, 93 open issues), home page <https://thingino.com> |
| Its own first line | "Thingino … is an **open-source firmware for Ingenic SoC IP cameras**" (`README.md`), and the site's `<h1>` tagline reads "Firmware for Cameras with Ingenic SoC" |
| Activity | `pushed_at` 2026-10-25; release `firmware-2026-10-04` carries **354 assets = 176 `.bin` firmware images** plus checksums — this is a live, shipping project, not a hobby repo |
| Build model | Buildroot **external tree**, like OpenIPC's `brandies`, but against its own fork (`themactep/buildroot`, branch `thingino` — `.gitmodules`). So the *shape* of the tree will look familiar to anyone who has read `docs/camera-sd-card-firmware-flash.md` |
| Branches | `master` = development, new **raptor** streamer, and a README warning that it "uses a **highly experimental mainline U-Boot**… unbricking skills is highly recommended"; `ciao`/`stable` = the release line, "the original ONVIF server and **Prudynt** with libconfig" |
| Image naming | `thingino-<vendor>_<model>_<soc>_<sensor>_<wifi>.bin`, e.g. `thingino-tplink_tapo_c110_t23n_sc3336_wq9001.bin` — the SoC is a field of the filename, which is what makes the support question answerable from the asset list alone |

```bash
# the three numbers above, re-read
curl -sS https://api.github.com/repos/themactep/thingino-firmware |
  python3 -c "import sys,json;d=json.load(sys.stdin);print(d['full_name'],d['license']['spdx_id'],d['pushed_at'],d['stargazers_count'],d['open_issues_count'])"
curl -sS "https://api.github.com/repos/themactep/thingino-firmware/releases/tags/firmware-2026-10-04" |
  python3 -c "import sys,json;a=json.load(sys.stdin)['assets'];print(len(a),'assets,',len([x for x in a if x['name'].endswith('.bin')]),'images')"
git clone --depth 1 --no-recurse-submodules https://github.com/themactep/thingino-firmware
```

**Lineage note, because `cameras/c110.md` §8.6 calls it "thingino (prudynt)":** Prudynt is
thingino's media daemon (`package/prudynt-t`, "what OpenIPC calls `majestic`") — the
parenthetical was about the streamer, not about a different project. `package/thingino-streamer`
makes the streamer a choice today: **prudynt** (what `ciao`/stable ships, with libconfig),
**raptor** (the default on `master`, with its own HAL/IPC/common packages), **strero**, **timps**.
All four sit on Ingenic's `libimp`, or on `package/openimp` as its replacement. Nothing here is a
fork of OpenIPC's tree.

## 2. The support question, answered from the tree rather than from a web page

Two independent reads, both reproducible offline in the clone:

**(a) Every device config in the tree is Ingenic.** `configs/cameras` (178 real boards),
`configs/cameras-exp` (57) and `configs/cameras-testing` (17, QEMU targets named `qemu_<soc>`)
hold **252** device directories, and **250 of them name an Ingenic SoC in the directory name**
(`t10l`, `t20l/n/x`, `t21n/z`, `t23dl/n/zn`, `t30l/x`, `t31al/l/n/x`, `t32lq/nq`, `t40nn/xp`,
`t41lq/nq`, `c20`, plus legacy spellings like `t21z` that `Config.soc.in` no longer lists). The 2
that do not: `cinnado_d1_3k_atbm6132u`, whose defconfig sets `BR2_INGENIC_SOC_MODEL="t41nq"`, and
`cameras-exp/vanhua_h53`, which is no board at all but an overlay of motor constants
(`steps_pan="3700"`) over `configs/cameras/vanhua_h53e_t31x_gc4023_eth_rtl8188ftv`. **Zero name a
SigmaStar part.**

```bash
for d in cameras cameras-exp cameras-testing; do ls configs/$d | wc -l; done   # 178 57 17 = 252 dirs
grep -rliE "ssc3[0-9]" configs/ | wc -l                                         # 0 — measured today
grep -h BR2_INGENIC_SOC_MODEL configs/cameras-exp/cinnado_d1_3k_atbm6132u/*_defconfig
grep -rh BR2_INGENIC_SOC_MODEL configs | sort -u | wc -l                          # 29 distinct, all t*/a1*/c100
grep -rl BR2_SIGMASTAR_SOC_MODEL configs/ | wc -l                                 # 0 — no board selects SigmaStar
```

**(b) The release images agree, and the scan is honest about its own noise.** Every one of the
**176** `.bin` assets of `firmware-2026-10-04` carries an Ingenic SoC field in its name (`t23n` 47,
`t31x` 42, `t31l` 34, `t21n` 15, `t31n` 13, `t20l`/`t20x` 4 each, `t10l`/`t30x` 3 each, `t21z`/`t31al`
2 each, plus singletons) and **not one names a SigmaStar part**. Two artefacts of the obvious
"split on `_`, take the third-from-last field" tally are worth naming, because a naive run prints
them and they are not findings: three filenames have a different field count, so that rule reads
`c20` (`thingino-hamedal_c20_t31a_gc2053.bin`), `gc4023`
(`thingino-vanhua_h53e_t31x_gc4023_eth_rtl8188ftv.bin`) and `106`
(`thingino-wansview_106_t31n_jxf37.bin`) where the SoC is really `t31a`/`t31x`/`t31n`; and a
SigmaStar-shaped scan has to exclude `hi3881`, which is a **WiFi module** on
`thingino-hc_3301ymt1_t31n_sc2332_eth+hi3881.bin` and not a SoC. The pattern form below has
neither artefact:

```bash
curl -sS "https://api.github.com/repos/themactep/thingino-firmware/releases/tags/firmware-2026-10-04" > /tmp/th.json
python3 - <<'PY'
import json, re
d = json.load(open('/tmp/th.json'))
n = [a['name'] for a in d['assets'] if a['name'].startswith('thingino-') and a['name'].endswith('.bin')]
ing = re.compile(r'[-_](t\d\d[a-z0-9]{0,3}|a1[a-z]{0,2}|c100)(?:[-_.]|$)')   # Ingenic model space
sig = re.compile(r'(ssc\d|sstar|infinity\d|nt9|novatek)', re.I)               # SigmaStar parts
print(len(n), 'images;', len([x for x in n if ing.search(x)]), 'with an Ingenic SoC field;',
      len([x for x in n if sig.search(x)]), 'with a SigmaStar-shaped token')
PY
# measured 2026-10-25: 176 images; 176 with an Ingenic SoC field; 0 with a SigmaStar-shaped token
```

**(c) And the SoC space confirms it.** `Config.soc.in` offers a **vendor choice** —
`BR2_SOC_VENDOR_INGENIC` (default) or `BR2_SOC_VENDOR_SIGMASTAR` — with ~50 Ingenic models on
one side and **exactly one** SigmaStar model on the other:

```
default "infinity6e" if BR2_SIGMASTAR_SOC_MODEL = "ssc30kq"
```

Our chips are `ssc333` and `ssc337`, on the family our own devices call **infinity6b0**. Not
`ssc30kq`, and not `infinity6e`.

```bash
grep -n -A4 "SIGMASTAR_SOC_MODEL" Config.soc.in | head -20
```

## 3. The part that makes this worth writing down: thingino *started* a SigmaStar port

This is why "thingino is Ingenic-only" is a sentence with an expiry date, and why the ⚠️ in
`cameras/c110.md` §8.6 needed replacing rather than repeating. Between 2026-08-03 and
2026-08-24 master grew a **vendor seam** in seven merged PRs (`johnchia`'s branch is named in
#1404's merge commit; the rest are the maintainer's):

| PR | Merged | What it did |
| --- | --- | --- |
| #1391 | ✅ 2026-08-03 | `soc`: declare the SoC vendor as a choice |
| #1404 | ✅ 2026-08-05 | `soc`: declare the sigmastar soc model (`ssc30kq`) |
| #1397 / #1398 | ✅ 2026-08-06 | move the Ingenic selects into a per-vendor block; `thingino-raptor-hal` owns the vendor selects |
| #1405 | ✅ 2026-08-05 | template the target architecture into the toolchain fragment URLs |
| #1426 | ✅ 2026-08-09 | replace `soc_database.txt` with one makefile fragment per SoC |
| #1443 | ✅ 2026-08-13 | read the SoC model from whichever vendor declares it |
| #1428 | ✅ 2026-08-24 | `thingino-system`: a `soc`/`sensor`/`usb-role`/`S03mac` file per vendor |
| **#1424** | ❌ **closed unmerged 2026-08-16** | **"sigmastar: add the infinity6e platform"** — the board layer, `package/sigmastar-{lib,sdk,uboot}`, the config fragments, `soc/sigmastar/infinity6e.mk` |

**The seam is in; the platform is not.** On master today: `ls board` → `ingenic`;
`ls soc` → `ingenic`; `ls kernels` → `ingenic.mk`; `ls package | grep sigma` → empty. Since
`thingino.mk` does `-include $(BR2_EXTERNAL)/kernels/$(SOC_VENDOR).mk` and errors with
*"no soc/$(SOC_VENDOR)/\*.mk claims it"* otherwise, a SigmaStar defconfig cannot resolve, let
alone build. PR #1424's own body says as much: *"**Nothing builds this board yet, and that is
deliberate.** There is no camera defconfig in this PR… The defconfig comes in a later PR once
the streamer backend is in place."* And upstream's own tooling still carries the future tense:

```
# scripts/make-bundle.sh, master f083382, line 57-63
# TODO: generalize for non-Ingenic vendors. … When the SigmaStar platform lands, derive
# SOC_MODEL from BR2_SIGMASTAR_SOC_MODEL when BR2_SOC_VENDOR_SIGMASTAR=y …
```

```bash
ls board soc kernels; ls package | grep -i sigma
grep -n 'no soc/' thingino.mk; sed -n '55,65p' scripts/make-bundle.sh
curl -sS https://api.github.com/repos/themactep/thingino-firmware/pulls/1424 |
  python3 -c "import sys,json;d=json.load(sys.stdin);print(d['state'],d['merged_at'],d['updated_at'])"
```

**And the SigmaStar machines thingino knows about are catalogued as *not supported*.** Its
`docs/brands/*.md` hardware tables list SigmaStar SKUs — `SSC333` 19 times, `SSC337` 16,
`SSC335` 12, `SSC337DE` 6 — with no support column, no image and no defconfig behind them.
Our exact chip is in there: `docs/brands/zte.md` rows `K540`/`K543 … SSC337 … 25Q128`. The
upstream issue that settled it is 22 months old and titled for the point:

> **issue #370 (2024-12-13): "ZTE K540 does not use Ingenic SoC"** — "The K540 does not use
> Ingenic at all. All chips do not match. SoC is Sigmastar SSC337DE. Wifi is MT7601UN."

So: on the project's own terms, an `ssc337` camera is a *don't-ask-us-to-build-you-an-image*
machine. There is no half-open door here to walk through — there is a Kconfig symbol for a
different chip and a closed pull request.

## 4. What the fleet actually is (measured, so the mismatch is stated both sides)

| device | SoC / family, on the device's own words | thingino? |
| --- | --- | --- |
| `cam5`, `cam9` | `ssc337 (infinity6b0 family), sc3338, 8 MB nor flash` (signature line, 2026-10-25) | **No** — `ssc337` appears nowhere in thingino's tree except its unsupported-brand tables |
| `cam7` | `ssc333 (infinity6b0 family), sc3338` | **No** — same |
| `cam1` | **now partly known**: RTSP `Server: TAS-Tech Streaming Server V100R001`, HTTP `Server: Ginatex-HTTPServer` (302 → `/index.asp`, `/login.cgi` → 401), telnet `:23` open, `:85 :8080 :8000 :10554 :37777` closed | **No** — that banner pair is the Xiongmai-family application stack (an inference from two banners, not a chip read: nothing here has seen cam1's SoC), and it is not Ingenic either way. The one `*.int` camera this repo has never identified is now fingerprinted by two banners |

`cam1`'s banners are new (read today, no credentials used, unauthenticated `OPTIONS`/`GET`
only), and they answer the "*what firmware is cam1*" row of `cameras/README.md` better than
"unknown" did. **They also opened a fault**: `DESCRIBE rtsp://10.3.2.6:554/0/av0` **and**
`/1/1` both answer **`200` with no authentication**, where cam5/cam7/cam9 answer
`401, realm WallyWorld`. By this fleet's own rule — *"a `200` from `:554` is a finding, not a
win"* — **cam1's picture is readable by anything that can reach VLAN 226.** Filed in
`./follow-ups.md` and in `../cameras/README.md` as fault 13; it is a bigger deal than cam7's
open admin, because cam7's open door is the *web UI* and this one is the *video*.

```bash
curl -m 6 -sS -i -X OPTIONS rtsp://10.3.2.6:554/ | head -4      # Server: TAS-Tech …
curl -m 6 -sS -i http://10.3.2.6/ | head -3                      # Server: Ginatex-HTTPServer
curl -m 6 -sS -o /dev/null -w '%{http_code}\n' -X DESCRIBE rtsp://10.3.2.6:554/0/av0   # 200 = ungated feed
```

## 5. The alternative that is not a maybe: OpenIPC still ships exactly these boards

The reason "not thingino" is not a shrug is that the thing we *can* do is already staged. The
manifest `just cam-image <soc>` reads answers for both chips in this fleet **today**, build
`nightly-20261024-856f22e` (2026-10-24):

```
nightly-20261024-856f22e ssc333_lite      nor 7137528 …openipc.ssc333-nor-lite.tgz
nightly-20261024-856f22e ssc337_lite      nor 7137552 …openipc.ssc337-nor-lite.tgz
nightly-20261024-856f22e ssc337de_lite    nor 7137533 …   (and ssc333_ultimate, ssc337_ultimate)
```

against a fleet running `master+9371591, 2024-08-21` (cam5, cam9) and a 2023.02.7 build
(cam7). `cam-preflight` passes 9 PASS / 1 UNKNOWN / 0 FAIL on cam7, flash dumps and `/etc`
archives exist for all three, and the fit gate is measured: rootfs 5,136,384 B inside a
5,242,880 B partition. **That is the decision with a receipt; thingino is a curiosity.**

```bash
curl -sS -m 60 -o /tmp/manifest.flat https://openipc.github.io/firmware/manifest.flat
grep -E " ssc33[37](de)?_(lite|ultimate) nor " /tmp/manifest.flat | sort | tail -6
```

## 6. What thingino *is* good for here

**6.1 — the next camera, if it is Ingenic.** If a future purchase is an Ingenic T21/T23/T31/T33/T41
board, thingino is the **better-maintained** route and the calculus flips: 176 per-board
prebuilt images (each with a `.sha256sum` beside it — checksummed, not signed), a documented
8 MB NOR "minimal installation" tier
(`docs/firmware/firmware-image-structure.md` §Flash Size Considerations), a stable
`ciao`/`stable` release line, and a hardware list you can match by SoC+sensor+WiFi before
buying. Relevant to this fleet's own history: the **Tapo C110 v3 is Ingenic T23N** and
thingino ships it (`thingino-tplink_tapo_c110_t23n_sc3336_wq9001.bin`,
<https://thingino.com/cameras/45> = "TP-Link Tapo C110 — T23N, SC3336, WQ9001"), which is what
`cameras/c110.md` §1's version tree claimed and what is now confirmed. **Read that sentence
carefully when buying**: the same retail model name spans a SigmaStar v1/v2 (OpenIPC's) and an
Ingenic v3 (thingino's), and the wrong image on the wrong board is the brick this fleet has
been careful enough to avoid for two years. `cameras/c110.md` §8.6's claim that thingino does
not support *this* board is correct; the reason is the chip, not the model name.

*One correction while we are here:* thingino's own `docs/hardware/supported-hardware.md` lists
**no Tapo C110** at all (only C100 v5, three lines), while the release carries `c100`, `c110`,
`c200`, `c200c`, `c210`, `c500`, `tc70` images. The generated web list is ahead of the
markdown one — so cite the **asset** and the camera page, not `supported-hardware.md`, when
arguing that a board is supported.

```bash
curl -sS -L https://thingino.com/cameras/45 | sed -e 's/<[^>]*>/ /g' | tr -s ' \n' ' \n' | grep -A5 "Tapo C110"
grep -n -i "tapo" docs/hardware/supported-hardware.md      # only C100 v5 — the list is incomplete
```

**6.2 — reading worth stealing, whatever firmware these boxes run.** Four things in thingino's
docs address failures `cameras/README.md` records as *normal behaviour* on this fleet, and
none of them needs a firmware change to copy the idea:

- `docs/media/rtsp-stress-test.md` + `package/thingino-rt-tests` — a **stress harness for the
  RTSP server**. Our cameras' normal fault is `majestic` watchdog self-restart
  (`watchdog_on=1`, `watchdog_timeout=10`) and `Sensor panic! → Camera is about to reboot`,
  with the log ring in `/tmp` so the evidence is gone by the time anyone asks. A repro that
  hangs a feed on purpose is the tool that turns that into a diagnosis.
- `docs/thingino/connectivity-watchdog.md` — a watchdog for *reachability* rather than for the
  media process, which is the half our fleet delegates to gatus rows on two of four names.
- `docs/dev/realtime-scheduling-for-embedded-services.md` — SCHED_FIFO/priority notes for
  exactly the class of stall we see at 5 fps @ 720p on a 3-year-old SigmaStar.
- The `send2*` family (`docs/thingino/send2telegram.md`, `send2webhook.md`, `send2ftp.md`) and
  `thingino-privacy-lite` (a video-privacy FIFO that forces a black frame without stopping the
  ISP). Our own Telegram angle on this fleet is a *leaked vendor bot token in a crontab*
  (`cameras/README.md` fault 1) — which is what happens when a notification path exists with
  no owner.

Treat these as reading for `crates/camera-monitor/` (the Rust replacement filed in
`./follow-ups.md`) and for whatever we eventually push at the fleet, not as a migration plan.

## 7. What a migration would cost, if a SigmaStar thingino ever appeared

Recorded so the next person does not have to work it out from scratch, and so nobody mistakes
"thingino supports the Tapo C110" for "thingino supports our cameras":

- **Different media stack.** thingino runs Prudynt (`ciao`) or raptor (`master`), not
  `majestic`. Everything in `cameras/config/*/majestic.yaml` — including the three findings
  that only exist because we read that file (cam9's `video0` with no geometry at all; cam5 as
  the only device pinning `isp: sensorConfig: sensors/sc3338.bin`; the `watchdog:` block) —
  stops being a config file and becomes a translation exercise.
- **Different RTSP paths, so the whole pipe moves with it.** Our feeds are
  `rtsp://camN.int.leighhack.org:554/stream=0` (majestic style) into go2rtc inside frigate's
  container. thingino's documented URLs are `/stream`, `/video`, `/audio`,
  `/video/trackID=N`. A firmware change here is a change to services1's untracked
  `/srv/frigate/config/config.yaml` and the generated `/dev/shm/go2rtc.yaml` too — the fault 8
  that is still open, multiplied.
- **No `fw`/`sysupgrade` compatibility with our procedure.** `docs/camera-sd-card-firmware-flash.md`
  is written around OpenIPC images (`uImage.<soc>` + `rootfs.squashfs.<soc>` loaded by the
  env's own `uknor`/`urnor` at `kernaddr=0x50000`/`rootaddr=0x250000`, fixed-slot layout,
  `sf write` writing `${filesize}` bytes). A thingino image is a single assembled `.bin` with
  its own partition table (`docs/firmware/firmware-image-structure.md`), i.e. a different
  write, and thingino's `master` line **ships U-Boot changes** behind a warning about
  experimental mainline U-Boot — the one region our procedure has always refused to touch
  (`ubnor` erases boot *and* env, and `bootcmd` lives there).
- **The vendor blobs problem does not go away, it changes vendor.** OpenIPC ships the
  SigmaStar `libimp`/`MI_*` userspace and sensor IQ files; thingino's SigmaStar PR would have
  added `package/sigmastar-sdk` "kernel side — MI modules, sensor drivers, tuning and VENC
  firmware". Whoever carries those, we do not get to pretend a firmware swap is only software.

## 8. Re-answering this question later (the whole thing is four commands)

The claim "thingino is Ingenic-only" is true at `f083382` (2026-10-23) and needs re-reading if
any of the four goes false. Nothing here should be inherited as a fact for more than a season.

```bash
# no clone survives this pass on any machine here; the re-check starts with it
git clone --depth 1 --no-recurse-submodules https://github.com/themactep/thingino-firmware /tmp/thingino-firmware
cd /tmp/thingino-firmware
# 1. does its Kconfig know our chips?            (today: only ssc30kq / infinity6e)
grep -n "ssc3\|infinity6" Config.soc.in
# 2. did the board layer land?                   (today: ingenic only)
ls board soc kernels; ls package | grep -i sigma
# 3. is there a defconfig for an ssc333/ssc337 board? (today: none in 252)
grep -rliE "ssc333|ssc337" configs/ || echo none
# 4. did #1424 get a successor, and is it open?  (today: closed unmerged 2026-08-16)
curl -sS "https://api.github.com/search/issues?q=repo:themactep/thingino-firmware+sigmastar+type:pr+state:open" |
  python3 -c "import sys,json;d=json.load(sys.stdin);print(d['total_count']);[print(i['number'],i['title'],i['html_url']) for i in d['items']]"
```

**Re-open this question only if:** (1) `Config.soc.in` gains `ssc333` or `ssc337` **and** (2) a
`configs/**` defconfig names a board with that chip **and** (3) a release asset exists for it.
Two of three is not support — that is exactly where the SigmaStar seam sat for the whole of
2026-Q3, and thingino's own brand tables have listed `ssc337` since 2024.

## 9. What changed in the repo because of this pass

- `cameras/c110.md` §8.6 and §1's version tree: the ⚠️ guess replaced with the measurement and
  a link here. Its conclusion was right and its filename is a real asset; what was missing was
  the *reason* (the chip, not the model name) and the expiry date (PR #1424). Two things fell
  out of checking it: thingino's own `docs/hardware/supported-hardware.md` lists **no Tapo C110**
  even though the image ships, so cite the asset or the camera page and not that file; and
  c110.md's *own* version tree puts `ssc337` on the **v1** branch — which is where cam5 and cam9
  report sitting, so "yours is a v2" only describes cam7.
- `cameras/README.md`: a bullet in the firmware summary (thingino is not an option, one line,
  the pointer here), `cam1`'s row and its *not proven* bullet filled in with the two banner
  fingerprints, and **fault 13** — the ungated `DESCRIBE` on cam1.
- `docs/camera-sd-card-firmware-flash.md`: one line at the top of the "which firmware"
  decision so the two docs cannot be read apart.
- `docs/follow-ups.md`: cam1's unauthenticated feed (new, security), the thingino re-check
  trigger, and the `just cam-probe` comment that carried a second, wrong copy of the feed
  table (now a pointer — `cameras/README.md` owns it).
- `AGENTS.md`: the camera row of the *read before you touch* table now points here, so the
  question gets measured before it gets answered from memory. `just check-eval` (all four
  assertion evals + `alejandra --check`) passes with these changes; nothing in them is
  deployed, and no `.nix` file moved.

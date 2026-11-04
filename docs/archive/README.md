# `docs/archive/` — documentation the fleet no longer acts on

An archived file is **frozen**. It keeps every word it had when it arrived here, including
claims later falsified, because the record of what was believed is the thing being kept. An
archive entry gets exactly one edit on the way in: the banner at its top naming what
superseded it and the date. After that, nothing — no link repair (relative links inside these
files point at where they lived, and `common/check-doc-pointers.nix` deliberately does not
scan this subdirectory, so a dead `./something.md` in here stays dead), no refresh, no
"current now" appendices. If an archived fact is needed live again, it goes into a live doc
with a new measurement; it does not unfreeze the archive.

## Why these files left the live set (2026-11-04)

**The C110 and its firmware are documented in the firmware's own repository**, where the code
they describe lives and where whoever reads them already is:

```
git@github.com:cjdell/thingino-firmware.git    branch ssc333-infinity6b0    PRIVATE
```

Private on purpose — the camera overlay commits the baked Wi-Fi PMKs; read
[`../camera-thingino-build-deploy-2026-10-31.md`](../camera-thingino-build-deploy-2026-10-31.md)
§7 before touching that visibility or assuming those PSKs are secret. Clone with
`git submodule update --init --recursive`, **plain, never `--remote`**.

What that repo owns, and therefore what this one stops restating:

| Question | Where it is answered now |
| --- | --- |
| What is a Tapo C110 (v1/v2 SigmaStar, v3 Ingenic), and what is on its 8 MiB flash | `docs/cameras/tapo-c110.md` |
| Flashing one from stock with a programmer, the rescue-card path, what to verify, recovery from a bad payload | `docs/cameras/tapo-c110.md` |
| Updating a running unit: SD rescue, `thingino-sysupgrade` semantics (it erases `data`), `make ota`, back to stock | `docs/cameras/tapo-c110.md` |
| Building the image: make goals, the container build, the SigmaStar `pack`/repack trap, `build-container.sh`, host-dep checks | `AGENTS.md` at the root of that branch |
| What the port has learned since this repo's pin: the speaker/audio bring-up and the `prudyntctl` shim, the night-latch fixes, the Wi-Fi reconnect watchdog, the repack/container traps | `docs/cameras/tapo-c110.md` and `git log c110-fleet-20261031..ssc333-infinity6b0` — 30 commits in that range, 20 of them the replays of pre-pin work the standalone re-creation produced, 10 genuinely new |

What stays **here**, and is not in that repo at any commit: the fleet itself (addresses,
MACs, frigate feeds, `common/dns-records.nix`), the pins and artifact hashes
(`cameras/thingino-manifest.sh`), the verbs (`just cam-fw-*`, `cam-rescue-card`,
`cam-migrate`, the `cam-*` OpenIPC set), the OpenIPC-side flash procedure and its audit
(`cam7`/`cam9` still run OpenIPC and the firmware repo does not describe it), the changeover
rule and the network-facing consequences of it
([`../camera-thingino-migration-2026-10-31.md`](../camera-thingino-migration-2026-10-31.md)),
which radio cell each camera joined
([`../camera-wifi-cells-2026-10-31.md`](../camera-wifi-cells-2026-10-31.md)), and the security
history of the PSK exposure (build-deploy doc §7, and the open rotation item in
[`../follow-ups.md`](../follow-ups.md)).

## What is in here

- **`c110.md`** — *"TP-Link Tapo C110 v2.0 — Complete Hacking Guide"*, the bench document that
  came with the fleet (stock SPI-NOR dump, boot chain, stock firmware inventory, the app-less
  provisioning routes, the OpenIPC reflash route, CVE claims, dump hygiene). It lived at
  `cameras/c110.md` until 2026-11-04. Superseded as instructions by
  `docs/cameras/tapo-c110.md` in the firmware repo; kept because the stock-firmware inventory
  and the boot-chain analysis are the only written record of what the 8 MiB dump contained, and
  nothing in it is actionable now that no camera in the fleet runs stock firmware.
- **`camera-thingino-evaluation-2026-10-25.md`** — the measured answer to "should these
  cameras run thingino?", which was *no, thingino is Ingenic-only*. That answer is dead:
  the branch above is the SigmaStar port, and `cam5`/`cam8` ship its images. Kept because it
  is the record of what upstream said at `f083382` and why the port had to be ours — and
  because its §8 four-command re-check is still how you answer the same question about **any
  other** firmware.

## Adding to it

Move the file with `git mv`, give it the banner (date, what superseded it, why it is here),
then fix every pointer in the live set in the same commit — `AGENTS.md`, `cameras/README.md`,
the justfile comments, `docs/follow-ups.md`, and the live docs that linked it. An archive entry
nothing points at is how a doc becomes invisible: that is the failure
`common/check-doc-pointers.nix` §3 was written against, and moving a file into this directory
opts it out of that check, so the pointers are the only thing left doing the work.

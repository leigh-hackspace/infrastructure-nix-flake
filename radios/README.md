# `radios/` — the 802.15.4 radios of this network

One file here is **data this network owns**: `firmware-pins.json` — the
OpenThread RCP and Zigbee-Router firmware this flake could ever put on a
dongle that is *here*, one row per artifact, pinned by URL and digest, with a
`flashed` flag that answers whether this network has ever put it on a dongle.
Nothing Thread is on this network today, and every row says so.

Everything the firmware itself does is upstream's, deliberately, and none of it
is written down here. AGENTS.md rule 30's shape, applied to the radios instead
of the cameras: the OpenThread tree and Koenkk's adapter repositories own what
an RCP build is, what an adapter board's flash holds, and which chips have how
much RAM; this flake owns the **network** — which dongle is on which USB node,
which channel its stack runs, which container address it listens on, which
VLAN it dials, which digests are pinned for this hub. A second copy of
upstream's firmware facts in this repo is how a table becomes a fiction.

| path | what it is |
| --- | --- |
| `firmware-pins.json` | DATA (the pins; `just thread-pins` prints it, `just thread-pin-digests` re-derives the digests from it) |
| `fw/` | **git-ignored**: the downloads go to `~/firmware-images` (outside the flake source — `just cam-image`'s measurement, restated for an RCP: a binary under the source is a path every evaluation copies into the store) |

and the three of `common/` that are about these dongles:

| path | kind | what it is |
| --- | --- | --- |
| `../common/radio-dongles.nix` | DATA | one row per dongle: `role`, `/dev/serial/by-id/...`, `mountAt`, `channel` |
| `../common/radio-tables.nix` | HELPER | the predicates over both tables |
| `../common/check-radio-tables.nix` | CHECK | the rules, fired from `just check-eval`, from `flake.nix`'s `checks`, and as a `config.assertions` of `zigbee2mqtt.nix`, `thread-router.nix` and `matter-server.nix` |

The verbs are `just/radios.just`: `radio-json`, `radio-status`, `radio-plug`,
`thread-pins`, `thread-pin-digests`, `thread-fw-fetch`, `thread-fw-verify`,
`thread-status`, and the two this session added: `thread-probe` (is a dongle
in a port an RCP radio at all — the pinned image, its own `otbr-agent --radio-version`,
the **uart** URL form, which is stronger than upstream's `forkpty` smoke test)
and `thread-flash` (digest-gated, refuses without `GO=1`). `docs/thread-border-router-2026-11-26.md` owns the evidence.

## The CC2652P: what its own firmware measures, and what it is still refused for

Measured 2026-11-26 off the fetched artifacts (`just thread-fw-fetch` into
`~/firmware-images`, `just thread-fw-verify` matched every digest against the
pins in `firmware-pins.json`):

| artifact | zip | inner |
| --- | --- | --- |
| `ot-ti-cc2652p-launchpad` (`CC1352P2_CC2652P_launchpad_ot_rcp_2025_3_1`) | 92 423 bytes | 228 114 bytes of Intel-HEX: **81 068 data bytes** (79 KiB), highest linear address `0x58000` |
| `ot-ti-cc2652r7` (`CC2652R7_ot_rcp_2025_3_1`) | 91 600 bytes | 226 457 bytes of Intel-HEX: **80 480 data bytes** — the *same* content — up to linear **`0xb0000` = 720 896, twice** that flash |
| `silabs-mg21-zbdonglee-rcp` (`.gbl`) | 110 540 bytes | — |
| `silabs-mg24-rcp` (`.gbl`) | 125 180 bytes | — |

What that measures, in the order it decides:

- **The CC2652P-class RCP image fits its own dongle, with room to spare**: 81
  068 data bytes (79 KiB) of flash against the class's 352 KB, and the R7-class
  build carries **80 480** data bytes — the same content — but reaches linear
  `0xb0000` = 720 896, **twice** that flash. The cliff is real, and it is a
  **link address**, not a byte count: both artifacts carry ~80 KiB of content,
  and one is linked for a memory layout a CC2652P-class dongle does not have.
  Do not re-derive the cliff from an image's size in a zip.
- Where it comes from, upstream: **RAM**. Koenkk's own `docs/adapters.md` gives
  the CC2652P class 80 KB RAM / 352 KB flash against the EFR32MG24 and the
  CC2652R7 classes' 256 KB / 1536 KB, and its issue tracker #4 reports the
  correct launchpad image on the correct ZBDongle-P dying with
  `RadioSpinelNoResponse` / "no response from RCP during initialization" the
  moment a device joins — a signature the TI e2e forum reproduces on Thread-certified
  R7-class TI silicon after ~6 sensors, which is why it reads as a size cliff
  and not as a broken radio.

**Corrected 2026-11-27: a CC2652P-class dongle has been made to work as a Thread
radio, by the sibling fleet, on the launchpad artifact above.** `otbr-agent --radio-version
answered `OPENTHREAD/1.4.0-Koenkk-2025.3.1; CC13XX_CC26XX` against a stick marked
"C2652P" on a CP2102N bridge, and an agent's first act against any RCP radio is
its Spinel init (`ot-br-posix/docs/adapters.md` line 34), so that line is the
radio answering its own identity question — `docs/thread-border-router-2026-11-26.md`
§13 owns the receipts and the probe command. So this network's spare Sonoff is
the `thread-rcp` row of `common/radio-dongles.nix`, and `just thread-flash` is the
verb that puts this pinned artifact on it. What stays open — and is why
`infra.thread.enable` is still false — is whether a **mesh join** holds on it:
upstream's #4 report and the TI e2e reproduction say the radio stops answering
the moment a Thread device joins, on 80 KB RAM, and nobody has measured this
hub's own graph either way. `just thread-status`'s node and link-quality line is
that measurement, and it is the number to read before the flag, not after.
What is *also* still true is the Zigbee-Router half: Koenkk's `docs/adapters.md`
does not list the ZBDongle-P as a Zigbee-Router adapter, so a repeater flash on
this dongle is an unverified pin, and one row cannot be a Thread radio and a
repeater at once (`common/check-radio-tables.nix` refuses two roles on one device
node, and a repeater has no `mountAt` because no stack talks to it).

## What a flash does and does not decide, and the rule that keeps a dongle's role honest

**A flash is permanent per dongle, and it is not a one-way door.** Nothing
downgrades a firmware, but the rollback is a flash of the other artifact: a
C2652P-class stick flashed to RCP comes back as a Zigbee radio by flashing any
Koenkk Z-Stack launchpad *coordinator* hex back the same way — measured and
written down as the rollback in the sibling fleet's `scripts/flash-thread-rcp.sh`.
What cannot be rolled back is the **network** the flash joined: a Thread radio
that has formed a network carries that network's credentials, and a Zigbee
coordinator that has been re-keyed carries its mesh's. So the honest rule is
that a flash changes a dongle's role permanently, the rollback changes it back,
and neither changes the mesh built on top of it — which is why the table's
`role` is the fact that matters, and why `just thread-flash` refuses without
`GO=1`.

**A flash does not change what a dongle is to the kernel.** The USB vendor and
product ids in `/dev/serial/by-id` are the hardware's, not the firmware's —
OpenThread's RCP builds for the C2652 variants are built to be interchangeable
on the same adapter board, and Koenkk's ZBDongle-P zip flashes that dongle's own
id — so a flashed Sonoff still answers as a Sonoff Zigbee dongle. That is why the
table's **`role`**, not the device path, is the fact that survives a flash, and
why the guard refuses a plugged-in radio whose name and role disagree rather than
trusting the name.

## Re-verify with

```
just radio-status                  # table vs the USB ports vs the channels each stack uses
just thread-pins                   # the pinned artifacts, and whether upstream's tags answer
just thread-fw-verify <file>       # the digest gate, before any flash
just thread-probe /dev/serial/by-id/…    # is the stick an RCP radio at all (identity, not mesh)
just thread-flash /dev/serial/by-id/… GO=1   # the pinned RCP artifact, onto a by-id dongle
nix eval --impure --raw -f common/check-radio-tables.nix   # must print ok
```

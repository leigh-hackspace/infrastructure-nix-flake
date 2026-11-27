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
`thread-status`. `docs/thread-border-router-2026-11-26.md` owns the evidence.

## The CC2652P: what its own firmware measures, and what it is still refused for

Measured 2026-11-26 off the fetched artifacts (`just thread-fw-fetch` into
`~/firmware-images`, `just thread-fw-verify` matched every digest against the
pins in `firmware-pins.json`):

| artifact | zip | inner |
| --- | --- | --- |
| `ot-ti-cc2652p-launchpad` (`CC1352P2_CC2652P_launchpad_ot_rcp_2025_3_1`) | 92 423 bytes | 228 114 bytes of Intel-HEX: **81 068 data bytes** (79 KiB), highest linear address `0x58000` |
| `ot-ti-cc2652r7` (`CC2652R7_ot_rcp_2025_3_1`) | 91 600 bytes | 226 457 bytes of Intel-HEX: **80 480 data bytes**, highest linear `0xb0000` |
| `silabs-mg21-zbdonglee-rcp` (`.gbl`) | 110 540 bytes | — |
| `silabs-mg24-rcp` (`.gbl`) | 125 180 bytes | — |

What that measures, in the order it decides:

- **The CC2652P-class RCP image fits its own dongle, with room to spare**: 81
  068 data bytes (79 KiB) of flash against the class's 352 KB, and the R7-class
  build carries **80 480** data bytes — the same content size, linked for a
  higher address (`0xb0000` against `0x58000`). So the size cliff the field
  guides describe is **not** "the Thread image does not fit", and it is not
  measurable from an image's byte count at all. Do not re-derive it from one.
- Where it comes from, upstream: **RAM**. Koenkk's own `docs/adapters.md` gives
  the CC2652P class 80 KB RAM / 352 KB flash against the EFR32MG24 and the
  CC2652R7 classes' 256 KB / 1536 KB, and its issue tracker #4 reports the
  correct launchpad image on the correct ZBDongle-P dying with
  `RadioSpinelNoResponse` / "no response from RCP during initialization" the
  moment a device joins — a signature the TI e2e forum reproduces on Thread-certified
  R7-class TI silicon after ~6 sensors, which is why it reads as a size cliff
  and not as a broken radio.

So a CC2652P is refused as this network's Thread radio, and kept as a spare.
What it *is*, radio-wise, is a working 802.15.4 radio: what it lacks is
Thread certification and RAM, not radio-ness, which is why the role it can play
is a **Zigbee-Router** repeater (`common/radio-tables.nix`'s `zigbee-router`:
it answers on one of the mesh's channels, and it has no `mountAt` because no
stack talks to it) — and why that row does not exist here yet, because Koenkk's
`docs/adapters.md` does **not** list the ZBDongle-P as a Zigbee-Router adapter
at all, so any Zigbee-Router flash on this dongle would be an unverified pin.

## The one-way door, and the rule that keeps a dongle's role honest

**A flash is a one-way door per dongle.** Nothing in OpenThread's or Koenkk's
trees offers a downgrade path: once a dongle runs Thread firmware, Zigbee-Router
firmware goes back onto it only by flashing again. The table's `mountAt` and the
guard's channel rules are what keep a dongle's role visible *while it runs*;
after a flash there is nothing to un-flash to.

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
nix eval --impure --raw -f common/check-radio-tables.nix   # must print ok
```

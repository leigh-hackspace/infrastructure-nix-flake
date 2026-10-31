# `cam8` cannot stream: its WiFi transmit is ~350 kbit/s (measured 2026-10-29)

**Status: superseded at its headline, still useful at its measurements — read this box before
using anything below.** The ~350 kbit/s ceiling, the bitrate ladder that proved it was a bandwidth
wall, and the measurement method are all sound and were reused on 2026-10-31. Two things are not.
**The attribution: the fault was never behind the camera's antenna — it was the access point and
its channel**, and `cam8` was a client of AP1's original unit on channel 6, the channel three of
this network's four APs were also sitting on. That unit failed and was replaced on 2026-10-31 by
one that auto-selected channel 11, where `cam8` is now the only 2.4 GHz client; it streams properly
from that moment on, under a firmware this file does not mention and without the driver swap it was
waiting for. **And the one-directional framing that produced the attribution**: "transmit is
~350 kbit/s, receive is 716 KB/s" is what made a hardware PA look plausible, and the download
direction was never re-measured on the *failing* camera after the ceiling appeared.
Receipts, the AP-side retry counters, and the survey that counts 26 BSSs where `cam5` sits against
4 where `cam8` sits: **`docs/camera-wifi-cells-2026-10-31.md`**.

For the rule this file taught: a per-camera throughput ceiling is not a per-camera *fault* until
the AP's retry counters and a second client on the same radio have been read, and an asymmetry
claim needs the download direction measured too — which the table below never did, and which is
the single measurement that would have named the channel on 2026-10-29.

## What was asked, and the answer nobody expected

"Can we actually get smooth video? I can't even get a picture in VLC." Two separate things:

1. **The URL.** VLC on `rtsp://root:…@10.3.2.112` gets nothing because majestic only describes
   a stream under an explicit path. It has to be
   `rtsp://root:…@10.3.2.112:554/stream=0`. A bare host is not a weaker connection, it is no
   stream at all. (All four cameras are the same; `cameras/README.md` shows the path in every
   example but never says that omitting it fails.)
2. **The stream.** With the path it connects and decodes — and then runs at roughly **half
   real time**. That part is a real fault.

## The measurement

Frames decode, the picture is right (H.264, 2304×1296, 20 fps bursts), so the sensor and the
encoder are fine. What is not fine is the radio, **in one direction only**:

| direction | result |
| --- | --- |
| **receive** — 4 MB pushed into the camera over ssh | **4 MB in 5.589 s ≈ 716 KB/s** |
| **transmit** — 4 MB pulled out of the camera over ssh | **4 MB in > 90 s, i.e. < 45 KB/s** |
| the RTSP stream itself (independent of ssh) | 4 Mbit/s: 74 frames in 66 s (0.11× real time) |
| | 512 kbit/s: 320 frames in 58 s (0.55×) |
| | 256 kbit/s: 300 frames in 22 s (0.67×) |
| camera → its gateway, 6 packets | 1.1 / 3.0 / 11.7 ms, clean |
| camera → its gateway, 200 × 64 B at 10 ms | **54 % loss**, worst 1682 ms |
| camera → its gateway, 500 × 1400 B | `ping: sendto: No buffer space available` |

The ceiling is therefore about **350–400 kbit/s of transmit**, and video is almost entirely
transmit. Lowering the bitrate helps exactly as much as that ceiling predicts — 0.11× → 0.55×
→ 0.67× — so it is a bandwidth wall, not a config or a decoder problem.

`iwconfig` on the camera reports `Bit Rate=65 Mb/s`, `Signal level=-55…-66 dBm`,
`Link Quality=50-55/70`, so the *negotiated* rate is fine and the *measured* one is 150× worse.
That gap is retries/timeouts the counters do not show (`Tx excessive retries:3` throughout).

## What it is not

- **Not the laptop or its AP.** It sits on a different network from the camera — `WiFi Go
  Brrr`, 5240 MHz, `94:a6:7e:00:31:63` — while the camera is on `Leighhack`, 2437 MHz (ch 6),
  `44:D9:E7:91:69:11`. That adds about 20 ms, and it is worth knowing, but it is not the
  cause: laptop → gateway is 0.2 % loss over 500 large packets, and the camera → **gateway**
  test fails just as badly as camera → laptop. The fault is behind the camera's antenna.
- **Not the sensor or the encoder.** 2304×1296 H.264, frames are correct, and each burst runs
  at a full 20 fps before the TX queue backs up.
- **Not CPU.** `72.7 % idle` during a stall; the load average of ~10 is D-state kernel threads
  in `MI_SYS` timed waits (`CamOsTcondTimedWait`) — normal for this pipeline, and exactly the
  kind of number that sends you looking in the wrong place.
- **Not power save.** `Power Management:on` was the first suspect and was turned off
  (`iwconfig wlan0 power off`); TX did not improve, so it stays a good housekeeping default and
  not the answer.
- **Not the SD recovery or the overlay.** Nothing from `docs/camera-wifi-recovery.md` is in
  play — the overlay is 216 KB of 704 KB and the network path is the stock one.

For scale, the same laptop, same minute, pulled `cam5` at **1.38× real time** (10 s of video in
7.23 s). The laptop is fine; `cam8` is the outlier.

## The two candidates, and the test that separates them

> **Answered 2026-10-31, and the answer is neither.** Kept as written, because "we thought it was
> the driver or the PA" is the lesson. The discriminator that was missing is in the new doc: a
> broken transmit chain is *asymmetric*, and the failing camera's **download** was capped as hard
> as its upload (76 KB/s down, 61 KB/s up on `cam5`), with the AP retrying **50 %** of its own
> frames to a −47 dBm client, and the client's physical rate holding at 23–39 Mbit while its
> throughput sat at 0.49 Mbit/s. **Do not run the module swap below**: it changes a variable that
> is not in the fault, and one side of it risks a board oops to test what the controller's own
> counters had already excluded.

`cam8` runs `Linux 4.9.84 … Oct 10 2026` with the **2026** `ssw101b` driver
(md5 `e7b171da8ce5069fa67cab48b419257f`) and the **2024** `sc3338` sensor module
(md5 `e4984cd70cdea1e2cbe671c6fd3a5ec3`). The driver changed yesterday; the board's TX chain
has not, but it is a bench board with the shield off, UART wires attached and a power strip
next to it, and it has already survived one USB oops.

1. **Driver.** Load the 2024 `ssw101b_wifi_usb.ko` (it is inside the 2024 rootfs dump already
   on the camera's card — `mount -o loop,ro` works there) against the current kernel,
   `rmmod` the 2026 one first, restart the network, and re-run the transmit measurement. If TX
   recovers, the update caused it and the fix is to keep the older module.
2. **Hardware.** If TX stays at ~45 KB/s on both drivers, it is the board — antenna, PA, or
   placement — and no firmware change will help.

The risk in (1) is symmetric to what happened yesterday in reverse: the 2026 driver oopsed on
the 2024 kernel (`Unable to handle kernel paging request at virtual address 000040c0`), so the
2024 driver may oops on the 2026 kernel. That means a hung board and a power cycle, which is
why the test is not run unattended.

## Re-measuring

```sh
# receive (camera's RX) — should be hundreds of KB/s on any working board
dd if=/dev/zero bs=64k count=64 | ssh root@10.3.2.112 'cat > /dev/null'
# transmit (camera's TX) — this is the one that matters for video
time ssh root@10.3.2.112 'dd if=/dev/zero bs=64k count=64' > /dev/null
# what a viewer actually experiences
ffmpeg -rtsp_transport tcp -i 'rtsp://root:<pass>@10.3.2.112:554/stream=0' -t 20 -f null -
```

A stream that reports `speed=1x` or better is real time. `cam8` reports 0.5× at 512 kbit/s.

## What was changed on the camera while measuring, and put back

`/etc/majestic.yaml` `video0.bitrate` 4096 → 512 → 256 (to prove the ceiling), restored to
**4096** and majestic restarted; a temporary `/32` route on the laptop to bypass tailscale
(removed); WiFi power save turned off (runtime only, does not survive a reboot).

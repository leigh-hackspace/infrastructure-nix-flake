# The voice assistant: a browser page, cam8's microphone, and a speaker nobody turned on

**2026-11-16.** Asked for: a smart speaker in the space using `cam8` (which has a mic and a
speaker on its board), with the transcribed request forwarded to *the bot that already
answers in Slack* rather than a second brain, and a spoken answer played back. Plus, before
any of that is trusted in a room: **a web page so a phone or laptop can be the speaker
instead** — same pipeline, ordinary hardware.

What exists now is that, in the order that could be proven: the pipeline works end to end
with audio in from the camera and speech out to a file; the page is on
`whisper.int.leighhack.org`; the camera's speaker is **off**, by decision, and this file
says why and what it would take.

`crates/voice-assistant/README.md` is the design and the traps. This file is the receipts.

## What `cam8` actually is, measured before any of this was designed

Three numbers and one absence decided the architecture:

| | measured on `cam8` (10.3.2.11, thingino 2) |
| --- | --- |
| CPU | **1×** ARMv7 `rev 5 (v7l)` |
| memory | `MemTotal: 36136 kB`, `MemAvailable: 14528 kB` |
| RTSP | `max_clients = 2`, **one already held by go2rtc** |
| audio | `/dev/snd` **does not exist**, `/proc/asound` empty — SigmaStar MI only, driven by `rad`/`rac` |
| echo control | `rac status`: `aec: false`, `ns: false`, `hpf: false`, `agc: false` |

So: no compute on the camera (the demo's own trick — run the model in the browser — was the
alternative, and it turned out to be the better one), no ALSA (so nixpkgs'
`services.wyoming-satellite`, which is the obvious "don't write this yourself" answer,
**cannot run on it**: its `arecord`/`aplay` have no device), one spare RTSP session, and no
echo canceller anywhere.

The audio path that *is* there works, measured both directions:

* `rac record -d 3 /tmp/t.pcm` → `recorded 3.0s, 96640 bytes l16`, and `POST /inference`
  on the live whisper server transcribed it in **2.7 s** as `' (soft music)\n'` — i.e. the
  pipeline is right and the mic heard the space's stereo rather than anyone talking. That
  is the whole problem in one transcript.
* a 350 ms, 440 Hz PCM16 tone piped to `rac play - -r 16000` →
  `rac: playing <stdin> (pcm), 16000 Hz` / `rac: played 0.3s, 5600 samples`. The **software
  path is proven; whether anyone in the room can hear it is not** — that question went to a
  human and is still open, and it is why nothing is plugged into a speaker yet.
* go2rtc — frigate's own, inside its container on services1 — already restreams cam8 with
  `-allowed_media_types video+audio` and its SDP carries `m=audio 0 RTP/AVP 97` /
  `a=rtpmap:97 L16/16000` — **16 kHz PCM, the rate whisper wants**, from a producer that is
  already up. So the satellite reads `rtsp://127.0.0.1:8554/cam8` and spends none of the
  camera's two client slots.

## The route that was chosen, and the one that wasn't

`nixpkgs` at this lock has `wyoming-openwakeword`, `wyoming-piper`,
`wyoming-faster-whisper`, `wyoming-satellite`, `piper-tts`, `sherpa-onnx`. The
Wyoming/Home-Assistant route is real and would have been faster to stand up, and it was
rejected for the row above: **the satellite side needs ALSA and the camera has none**, and
its voice pipeline would be a second answer path parallel to the bot rather than a client
of it. Route A — a house-style Rust crate that reuses the pieces we already run — won:

```
  cam8 mic ──go2rtc restream──▶ voice-satellite ─┐
  browser mic ──────────────────────────────────┤
                    voice-gateway :8193 (loopback, no auth, so loopback)
                      ├ whisper.cpp        aibox :8082   (already deployed)
                      ├ slack-agent listen :8194          (NEW: the bot, on a socket)
                      └ piper en_GB-alba-medium           (NEW: pinned voice)
                      └ speaker: off → the WAV is written, nothing is played
```

`crates/whisper-ws` already had the browser half: `getUserMedia` → 16 kHz PCM → the
Realtime-API WebSocket → OpenWakeWord in WASM → a live transcript on a LAN page. The
assistant is a **card added to that page** (`AssistantCard.tsx`) that posts the same
recording to the gateway, so the phone gets push-to-talk, hands-free arming on the existing
wake word, a typed fallback, and an answer out loud — and the gateway URL comes from
`/status.json` (`--assistant-url`), not baked into the bundle, because a hostname compiled
into a SPA can only be changed by rebuilding it.

## The bot needed one endpoint, not a rewrite

The thing to know from the 2026-11-16 Slack-agent work held: `agent.rs` is the loop and
`slack.rs` is a transport, and the `ask` verb already ran the whole thing with no Slack.
So the generalisation is **`slack-agent listen`** (`src/listen.rs`): `POST /v1/ask` on
loopback, same engine choice, same MCP tools, same conversation reset, and two channels
that differ only in how the answer is shaped —

* `channel=text` keeps Slack's prompt and `MAX_ANSWER_TOKENS = 700`;
* `channel=voice` uses `prompt::spoken_system_prompt()` and **`MAX_SPEECH_TOKENS = 200`**
  — one or two sentences, no bullets, no URLs, no emoji, units as words, *start with the
  fact*. A test fails if the speech budget ever exceeds the Slack one, so the voice path
  cannot silently inherit the verbosity a wall needs.

`--mcp`/`--endpoints-file` are required there exactly as in `serve` (an agent with no tools
must not answer from memory), and the unit deliberately has **no `EnvironmentFile`** — this
face of the bot holds no token; the tools that read the building are a different process.

## The measured latency, since "it's slow" is not a finding

| step | measured |
| --- | --- |
| 3.0 s of camera audio → transcript | **2.7 s** (whisper `medium`, **CPU**: `whisper-server` holds zero `/dev/dri` fds, strata owns the iGPU) |
| one-tool question → answer | **7.7 s** (`qwen3.8-flash-next-coder-iq1m` on the RTX 3060, 2 rounds, `space_status`, 2794 in / 96 out) |
| two-tool question → answer | **15.3 s** (3 s of speech, `space_status` + `temperature`) |
| 31 words → 7.8 s of speech | **2.5 s** (piper per-request process — ~2.4 s of it is startup) |
| **spoken turn, end to end** | **~13 s** for a one-tool question |

The budget is the **model**, not whisper or piper — which is the finding, because it means
the fixes are the ones already in the tree (the deterministic `triage::events_window` path
that needs no engine, and a smaller whisper model on aibox) rather than a rewrite.

## What is deliberately *not* running, and the two things blocking it

1. **The camera's speaker.** `--speaker ssh` is written and unit-tested; the default is
   `off`, and `machines/services1/services/voice-assistant.nix` says so in its header. Two
   things must be true first: someone must have **heard** a C110 piezo across a working room
   (unproven — see above), and the unit needs an **identity it may read**: playing means
   `ssh root@cam8`, and the fleet's hop key lives in operator accounts where a `DynamicUser`
   cannot reach it, while copying a private key into the store is precisely what
   `infra.buildTimeSecrets` exists to make expensive. A dedicated `voice-speaker` account
   whose *public* key is installed on the camera is the clean answer and requires a device
   write, which is a decision, not a default.
2. **Barge-in.** No AEC on the camera and none on the server, so a speaker that talked while
   listening would transcribe its own answer into a new question. Half-duplex is enforced in
   `satellite.rs` (audio captured during playback is dropped, counted in `dropped`, gate
   reset after) and the numbers needed to do better — `audio_ms`, `played_at` — are recorded
   so server-side AEC against the playback clock is a possible next step rather than a guess.

One more, unfixed and named: `cam8`'s RTSP credentials are the firmware defaults
(`thingino:thingino`), in plaintext in `/srv/frigate/config/config.yaml` — off-flake runtime
state. As long as the camera is only *listening* through a restream that frigate already
holds, this is a pre-existing exposure of the video stream; treating it as a permanent
microphone makes it worth rotating, and the rotation has to land in the same change as
frigate's config, which is a runtime file with no flake owner (`cameras/README.md`).

## How to tell if it is working

```
just voice-status      # gateway counters + what the mic last heard, through the front door
just voice-ask "…"     # one question: answer, spoken text, WAV to /tmp, timings, tools, warnings
just voice-say "…"     # TTS only → /tmp/voice-say.wav. Plays nothing.
just voice-probe 8     # cam8 mic → gate → whisper → STOP. The measurement verb.
```

`voice-probe` is the verb this system needs most, because the one thing no amount of
desk work can answer is the room: how often its music, tools and ten conversations open the
speech gate and produce a transcript that looks like a question. Its counter is
`probe_false_positives` in `/status.json` and it is *deliberately* not an assertion — there
is no honest threshold until someone has watched it for a week. That is the gate criterion
for ever turning the speaker on: **run `just voice-probe` until you can say what its rate is.**

## Proven / guessed, as of this writing

Proven on these boxes today: the camera mic → whisper path (2.7 s); the camera's
`rac play` accepting PCM (as software); the bot's answer loop over MCP with tools
(`space_status` → "open since 18:40 UTC") in 7.7 s; the piper voice synthesising
16 kHz speech in 2.5 s; the full synthetic turn — a piper-spoken question, transcribed
back by whisper as `Hey Jarvis, is the hackspace open right now?`, answered by the bot,
spoken back to a WAV — with the camera's speaker switched off; `just check` green with the
new front-door row, the `voice` DNS label, and the origin vhost in agreement.

Not proven, and not claimed: that anyone can hear the camera's speaker; that the gate
behaves acceptably with the space's actual noise floor (only a real-room `voice-probe`
session answers that); that the browser page works from an HTTPS origin — the gateway
answers CORS for `https://whisper.int.leighhack.org` and that pairing has now to be
clicked through once on a phone; that a *resident* piper or a smaller whisper model is
worth the deploy. All five are in `docs/follow-ups.md`.

## The second half of the night: what the built binary found in its own code

Everything above was written against code I had tested with hand-run builds.  Then the
crane build finished, and running the **deployed** binary against the live camera produced
three findings in the order you would want them recorded.

**1. An open window at end-of-input was being thrown away.**  A gate window ends after
`hold_secs` (0.7 s) of trailing silence, so the utterance the gate was still holding when
the input stopped went nowhere: the end of a clip, yes, but also **go2rtc dropping its
session, a `probe --seconds 8` running out, and every ffmpeg restart**.  It surfaced as
*"the gate never opened"* on a clip that `gate-check` then showed opening at 0.38 s — a
diagnosis exactly as wrong as it sounds, which is why `gate-check` is in the release:
it prints the frame-by-frame table (level, floor, threshold, modulation, `hf`, `voiced`,
and which test said no), so "not hearing me" splits into *"the room was quieter than the
threshold"* and *"the threshold is above the room"*, which have opposite fixes.
`Gate::flush` now hands the held window to the caller at end-of-input with `flushed: true`,
logged as *"the stream ended mid-word, sending it anyway"*; `--min-secs` still filters it.

**2. The diagnostics lied, twice, in the same direction.**  `floor` and `threshold` were
refreshed only on a completed window or a five-second timer, so a short stream reported
their **initial 0.0 dBFS** — full scale, "something is clipping", about a silent room.  And
the peak was measured as the loudest *sample* while the gate compares *frame* RMS, so the
heartbeat and `gate-check` disagreed by 8 dB about the same clip.  Both fixed (refresh per
chunk; peak read from the gate), and the probe's summary now prints the sign of the gap and
the speech-frame count, because "0 windows" with 0 speech-like frames and "0 windows" with
400 of them are different bugs.

**3. Whisper does not write down what you said.**  The control experiment — a piper-spoken
"hey jarvis is the hackspace open right now" fed through the satellite's real path — came
back as `K Jarvis is the Hackspace open right now.`  The bare `jarvis` in the default wake
list is what saves that; **a wake list of only two-word phrases would be deaf to the phrase
it exists to hear**, and no unit test can see it because both sides of such a test are
spelling.  (The page's OpenWakeWord model does not have this problem — it works on the
audio.  It is the transcript-decision path that reads like a caption.)

Measured, `cam8`'s mic, an empty space at 00:28: **floor −48 dBFS, threshold −40, loudest
frame −43** — the gate correctly called it a quiet room.  The same defaults on the spoken
clip: median −20 dBFS, 62 of 135 frames speech-like, opens at 0.38 s.  Those are the two
ends of the working range on this hardware, measured rather than guessed, and
`just voice-probe` in a full room is the missing third point.

One deployment fact settled on the way: **frigate's go2rtc restream answers
`rtsp://127.0.0.1:8554/cam8` from the host, no credentials** (the `FRIGATE_RTSP_PASSWORD`
in `machines/services1/services/frigate.nix` guards the LAN-facing endpoint, and the
container's port publish lands the loopback path outside it).  So the satellite's default
`--source` works as written, and the camera's own two-session RTSP budget is untouched.

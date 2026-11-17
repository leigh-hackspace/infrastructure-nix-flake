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

## The next morning: the first person to use it was not the person who wrote it

Every end-to-end test above was mine, from a shell, with a WAV I generated. The first human
being to open the page found two bugs in under a minute, and neither was visible to anything
I had run — which is the whole argument for putting things in front of people.

> "is it press to talk or hold to talk? it doesn't seem to be either as i only get about half
> a second to say anything. arm wake word doesn't do anything and it causes a script error"

### Half a second, and why it was never going to be hold-to-talk

It is click-to-start / click-to-stop, and the label said "Press to talk", which is the phrase
everybody reads as *hold*. Fixed in the copy, because a control whose name implies a gesture
it does not implement is a bug report waiting to happen.

The half a second was real, and it was two separate faults stacked:

```ts
quietMs = db < QUIET_DB ? quietMs + (chunk.length / mic.sampleRate) * 1000 : 0;
if (quietMs > STOP_AFTER_QUIET_MS || ...) void stopTalk();   // QUIET_DB = -42
```

1. **Silence was counted from the instant of the click.** A click, a breath, a sentence — the
   take ended before the sentence started. Silence before you have spoken is not the end of
   your turn.
2. **`-42 dBFS` is a statement about the microphone, not the room.** A quiet input read
   around −49 dBFS *while someone was mid-sentence*, so every frame was "silence", the
   countdown ran in real time, and the take was cut at roughly a second whatever the person
   did. Any absolute dBFS cut in a voice path is a hidden dependency on input gain.

So the endpointing now works the way the server-side gate already does — **relative to the
take's own noise floor** — and silence only starts a countdown once a voice has been heard:

```ts
floorDb = seeded ? Math.min(floorDb, db) : (seeded = true, db);   // seed from frame one
if (db >= Math.max(SPEECH_FLOOR_DB, floorDb + SPEECH_MARGIN_DB)) { spoke = true; lastLoudAt = now; }
if (!spoke) { if (now - startedAt > NO_SPEECH_MS) void abortTake(); }
else if (now - lastLoudAt > STOP_AFTER_QUIET_MS || secs * 1000 > MAX_RECORD_MS) void stopTalk();
```

with `SPEECH_MARGIN_DB = 8`, `SPEECH_FLOOR_DB = -48`, `STOP_AFTER_QUIET_MS = 1200`,
`NO_SPEECH_MS = 5000`. And the two timers a person can feel are now read from
`performance.now()` rather than from a sum of chunk durations — a bug in the chunk accounting
must not be able to make the *deadline* lie about how long someone has been waiting.

A take that heard nothing no longer sends silence to whisper: it says so **with the numbers**
(`floor −54 / loudest −52 dBFS`), because "the input is too quiet" and "the room is too loud"
look identical from the user's side and are opposite diagnoses.

### The script error was a call on a data object, and it is my fault twice over

```
src/state/assistant.ts(250,14): error TS2349: This expression is not callable.
  Type 'ResultMsg' has no call signatures.
```

`untrack(wakeResult)()` — `untrack` **calls** its argument and returns the value, so this took
the `ResultMsg` and tried to call it. It threw on every click of the arm button, *after*
`setHandsFree(true)` had already run, which is why the pill lit and nothing happened. And a
click handler is not a tracking scope anyway, so the `untrack` bought nothing; `wakeResult()`
is the correct form.

The other half of the fault is the design: **arming only set a signal.** The wake-word card
above is what actually runs OpenWakeWord, so arming the assistant without starting *it*
produced a lit pill and no detector — "doesn't do anything" was accurate. `toggleHandsFree`
now goes through the card (`mode()`, `startWake()`, `stopWake()`), refuses to claim it is
armed if the card would not start, and says which status line to read when it cannot.

`tsc` also flagged `post('/v1/transcribe', wav(samples), …)`: since TS 5.8 a bare
`Uint8Array` is `Uint8Array<ArrayBufferLike>`, a `SharedArrayBuffer` is not a `BlobPart`, so
the honest signature is `Uint8Array<ArrayBuffer>`. It worked in the browser and was still a
type error.

### The gate that should have been there from the start

The page has had a `typecheck` script (`tsc --noEmit`) since the port, **in nothing that ran
it**: `buildNpmPackage` runs `npm run build`, which was `vite build`, and esbuild strips types
without checking them. So both bugs above — one of them a plain "this is not callable" —
compiled, shipped to aibox, and were live for half a day.

`package.json` is now `"build": "tsc --noEmit && vite build"`, so the *only* way this bundle
gets built is through the checker, in the derivation as well as in dev (verified: `nix build
.#whisper-ws` rebuilds and succeeds, so `typescript` is in the sandbox's `node_modules`; and
`npm run build` exits 2 with a deliberate error and never reaches vite).

A build step nobody invokes is not a test. It was not a lint I skipped either — it was a lint
that existed, in the repo, next to the code, and was simply never in the path.

## The wake tier: detection separated from transcription, and why the ear is a model

Asked plainly: *is a specialised model doing wake-word detection, or is it continuous
Whisper?* The answer was **both, in two places**, and the two-ears split is what made the
cam8 path wrong:

| ear | spotting | what Whisper saw |
| --- | --- | --- |
| the browser page | OpenWakeWord, three ONNX sessions in the tab (`melspectrogram` → `embedding` → `hey_jarvis` head), threshold 0.5, 2 s cooldown | only the utterance *after* the keyword |
| `cam8`'s satellite | **no model** — an energy gate, then Whisper transcribes **every** window, then `wake.rs` searches the *text* | everything that cleared the gate |

The second one is a design mistake wearing a plausible face. Whisper medium on aibox costs
**~900 ms of CPU per audio-second of gated speech** — roughly one stream per core, and it is
paid per sentence the room happens to say, not per request. It also stores a transcript of
everything the camera hears even when it was never addressed (`stats.last_transcript`), and
it makes wake detection hostage to ASR output: piper-spoken "hey jarvis" comes back from
Whisper as **`K Jarvis`**, which is the only reason bare `jarvis` is in the phrase list. Text
matching on a hallucination-prone transcript is a reliability ceiling, not a threshold to
tune.

So the tier is now: **detect cheaply and always, transcribe only what was aimed at us** —
with the seam designed for two classes of device, because a browser and a camera cannot be
given the same job:

```
class 1 — device detects locally            class 2 — dumb audio stream
browser (wasm), HA/Wyoming satellites       cam8 RTSP→go2rtc PCM, phones, ESP32s
   │ flagged utterance only                    │ continuous 16 kHz PCM
   ▼                                           ▼
voice-gateway /v1/transcribe  ◄── pre-roll ring ──  voice-wake  (Wyoming, loopback/LAN only)
   └► slack-agent listen → tools → piper              streaming scores, 80 ms steps
```

**Protocol: [Wyoming](https://github.com/OHF-Voice/wyoming)** (an Open Home Foundation
standard, and Home Assistant's own voice plumbing) rather than a bespoke RPC — one JSONL
header plus optional PCM, with `describe`→`info` (advertising `wake.models[]`,
`active_wake_words`, `max_active_wake_words`), then `detect {names}` +
`audio-start`/`audio-chunk{rate,width,channels}`/`audio-stop` → `detection {name, timestamp}`
or `not-detected`. Reusing it means an ESP32 satellite or Home Assistant can point at our ear
with a URI, and a device that detects for itself simply never appears at the wake server. It
has **no authentication or encryption by design** ("trusted network, must not be exposed to
the internet"), so it binds loopback/VLAN like everything else unauthenticated in this
stack — never the front door.

**Engine: [`livekit-wakeword`](https://github.com/livekit/livekit-wakeword)** (Apache-2.0) via
its Rust crate, on its published benchmark — same phrase trained by each engine with its own
recipe, `wakeword-bench` (7,873 clips of unseen voices/noise/rooms/codecs/near-misses plus
24 h of TV, music and podcasts), recall at 0.5 false alarms/hour: **LiveKit 89 %, nanowakeword
49 %, openWakeWord 46 %, microWakeWord 0 %**, at 16.5 / 30.5 / 29.5 / 2.2 ms of CPU per
audio-second. Porcupine was rejected on licence volatility rather than merit: Picovoice's
free-tier AccessKeys **stop working 2026-06-30**, and custom keywords mean uploading
recordings to their console. And `hey_jarvis.onnx` is published ready-made — the phrase this
fleet already uses — which matters because LiveKit's *training* path needs a CUDA GPU or Apple
silicon plus LiveKit Cloud TTS, neither of which aibox is.

The crate settles the two things that would otherwise have been hand-rolled: the melspectrogram
and embedding frontend (with its FIR resampler) and the per-model streaming state. It resolves
to the **pure-Rust `tract`** backend — `ldd` on the release binary is `libc`/`libm`/`libgcc`
and nothing else — so there is no ONNX Runtime blob for a Nix build to fetch. Measured here,
and the reason it is its own cargo workspace: `crates/voice-wake/README.md`.

**Deployed as detection only** (services1, `voice-wake.service` + the gateway's
`/v1/wake/*` proxy, and `whisper.int` now offers `wake: server` per tab): nothing answers,
nothing plays, no camera is wired to it. What is *not* earned yet is the threshold, and that
is a number from this room: our own busy-hour false-accept rate, with the threshold picked from it the way
Frigate's `min_area` floor was. Until that measurement exists, an always-on detector in a room
where people are working is a feature someone will mute — and the one measured result that
warns loudest is that piper's isolated "hey jarvis" scores **0.063** while the same phrase
inside a sentence scores **0.62**.

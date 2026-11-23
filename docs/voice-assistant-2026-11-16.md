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

## What it can be asked (added 2026-11-17)

The voice answers come from the same ten tools Slack sees (`docs/services.md`), seven of
them about the building and three — `what_time`, `calculate`, `convert` — that are the
reason a voice assistant is worth switching on at all: "what time is it", "what's 3/8 in
mm", "how many litres is three gallons". They were added because the prompt previously
told the model, in prose, that the UK is "UTC in winter, UTC+1 in summer" — an instruction
to do daylight-saving arithmetic in its head, on a quantised coder model, out loud.

Two measurements from that work, both of which changed code rather than only docs:

* **espeak-ng reads `2026-11-17` aloud as "…twenty sixteen *dash* eleven *dash*
  seventeen"** (`nix run nixpkgs#espeak-ng -- -v en-gb -x -q`, the same front end piper
  uses). So a tool that prints an ISO date is correct on a screen and says "dash" in a
  room. `what_time` now prints `Tue 17 Nov 2026 16:14 GMT`; the shared formatter is
  `clock::nice_date`, and the event list is still un-migrated (`docs/follow-ups.md`).
  Times themselves are safe: `16:10` reads "sixteen ten", with no "colon".
* **the basics trio needs no network**, which makes it the diagnostic in `--self-test`:
  when all eight building tools fail at 03:00, "the 3 tools that need no network all
  answered: this process is fine, the failures above are upstreams" is the difference
  between fixing Home Assistant and restarting a healthy server.

In Slack these are reachable **by question, not by command**: "what time is it" is
answered unmentioned, "convert 12 feet to metres" is not, because that is two members
talking at the bench. The gate is pinned both ways in `triage`'s suite. The voice path
never reaches that gate — a spoken turn is always addressed.

## Which detector, where — settled as far as it can be today (2026-11-17)

The report this section supersedes said the browser's detector "was a floor, not a ceiling"
and left the reason unexamined. Asked whether the reason was `wasm` — "why not run the wasm on
the server?" — the answer turned out to be measurable rather than arguable.

They are not two runs of one detector. From the code, not from memory: the page runs
openWakeWord as **four ONNX graphs** (`melspectrogram.onnx` → `embedding_model.onnx` →
`hey_jarvis_v0.1.onnx`, plus `silero_vad.onnx` as a gate), on 1280-sample (80 ms) frames from
an `AudioWorklet` tap, firing only when score > 0.5 **and** the VAD is open, with a 2 s
cooldown (`crates/whisper-ws/web/app/src/state/assistant.ts`). The server runs LiveKit's
**single** `hey_jarvis.onnx` on 160-sample (10 ms) internal steps, no gate, fed as ~0.5 s PCM
chunks over HTTP through `voice-gateway`. Model, gating, and the samples each one sees: three
variables, none isolated. `wasm` is how a browser gets to run ONNX at all — it is not a
property the server needs, since we already execute ONNX natively.

So `voice-wake oww-bench` runs the browser's own graphs natively, on the same `ort`+tract
runtime as the shipped engine, over the same sha256-pinned model files the page fetches
(`crates/whisper-ws/package.nix` → `nix-store -r` on the assets derivation). Same runtime, same
bytes, two models:

| | CPU per audio-second | listeners per core |
| --- | --- | --- |
| shipped LiveKit model (`bench`) | **32.1 ms** (3.2 % of a core) | ~31 |
| openWakeWord, native (`oww-bench --gate off`) | **287 ms** (28.7 % of a core) | ~3.5 |

Three findings, and the first is the one that changes the decision:

* **The browser's better-seeming detector costs ~9× the CPU per stream.** Invisible on the one
  laptop a person happens to have open; it *is* the design on a server holding many
  microphones. This is the arithmetic behind putting detection on devices that can afford it
  and giving dumb endpoints the cheap engine — and it is now a measured number rather than an
  intuition about ARM.
* **The gate does not port as shipped**: silero's ONNX export uses the ONNX `If` op, which
  onnxruntime-web runs and tract refuses to translate (`Failed to parse model: Translating
  node #72 "If_25" If`). So the part of the browser's pipeline most likely doing the work
  against music and room chatter is precisely the part that must be re-sourced — an older
  silero export, or our own energy gate — before any parity claim is testable. Op support, not
  runtime speed.
* **The model question is still open, because the audio I had was invalid.** openWakeWord
  scores 0.000 on piper-synthesised "hey jarvis" where LiveKit scores 0.626, and the 240 s room
  recording has no wake word in it (0.000 against LiveKit's 0.031 — usefully, proof both
  engines were fed the same bytes). Synthetic speech is out-of-distribution for one of the two,
  so that gap is about training data, not about detectors. Settling it needs recorded human
  speech from one microphone through both engines: the follow-up is in `docs/follow-ups.md`,
  and until then the 9× CPU figure is the only number here that stands.

Two measurement lessons from building it, both cheap to forget and expensive to trust:
`ndarray = "0.16"` silently pulled a **second copy** of ndarray (ort was built against 0.17)
and produced four trait errors on types that obviously implement the trait — same name,
different crate instance, fixed by matching ort's version; and `Instant::now().elapsed()`
measures from the instant it is called, which made the cost line print a flattering
`0.0 ms per audio-second` for a pipeline that had definitely run.

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
with a URI, and a device that detects for itself simply never appears at the wake server.

**Correction, read before quoting the paragraph above:** as deployed, **there is no Wyoming
TCP listener anywhere in this fleet.** `voice-wake serve` implements that *event model* over
loopback HTTP because a browser cannot open a TCP socket, and its header says so. So "an ESP32
satellite or Home Assistant can point at our ear with a URI" is the design note, not the
contract: what an ESP32 speaks today is `POST /v1/wake/session` + `POST /v1/wake/chunk` behind
the front door, and the Wyoming front end that would let a third-party satellite dial us is
unbuilt. The contract, the format, the VLAN question and the list of what does not exist are
in `docs/esp32-voice-satellites.md` — which is the file to read before writing device code, and
which restates none of this section's reasoning. It
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

## Streaming end to end (2026-11-22): the sum of the parts was the wrong number

Everything above measures the stages **one at a time**, and the deployed pipeline added them
up: transcribe the whole window, then ask, then synthesise the whole answer, then play.
~13 s, every number honest, the total still a choice nobody had made deliberately.

This section records the change to **overlap** them. The mechanism is
`crates/voice-assistant/README.md` ("The turn is a pipeline") — read that for the threads,
the channels and the two rules the pipeline enforces. What belongs here is the *why*, the
measured part, and the part that is still only a budget.

### What moved, in one line each

| | |
| --- | --- |
| `common-rs/web/src/stream.rs` | `Sse`/`Chunked`: flush per event, `TCP_NODELAY`, `Cache-Control: no-store`, `X-Accel-Buffering: no`. Its tests prove an event reaches the client **before** the next one is written — which is the property every later claim depends on, and which nginx would silently remove. |
| `common-rs/http/src/stream.rs` | the client half: `request_stream`/`read_event`, chunked + content-length + EOF-delimited bodies, SSE events reassembled across read boundaries, and an idle timeout that distinguishes *silence* from *the end of the stream*. |
| `slack-agent/src/stream.rs` | the engine half: SSE deltas forwarded as they arrive, tool-call fragments reassembled by `index`, keep-alives skipped, malformed `data:` lines **counted** rather than fatal, and `StreamFail::emitted` — the only way a caller can tell "retry" from "you have already shown the room those words". |
| `slack-agent/src/{engine,agent,listen}.rs` | `POST /v1/ask/stream` (SSE: `start`/`log`/`partial`/`reset`/`answer`/`error`) and `POST /v1/ask/prefill`; `ask_budget_stream` keeps the tool rounds, the budget and the grounding rules of the buffered `ask_budget` rather than forking a second answering path. |
| `voice-assistant/src/{asr,sentence,turn,events,turn_routes,gateway}.rs` | rolling ASR with overlap and seam dedup, the speakable-unit splitter, the pipeline, the wire contract, its HTTP surface, and the satellite's client of the gateway. |
| `voice-assistant/src/tts.rs` | a **resident** piper (`--output-dir`, one WAV per sentence, `Wrote <path>` on stderr as the frame boundary), with the cold one-shot call kept as a counted fallback rather than deleted. |
| `voice-assistant/src/sink.rs`, `drivers/` | `Sink::streams`/`play_stream`: a sink that can start playing on the first buffer, and one that cannot says so. The turn does not guess about a device. |

### Why the warm-up asks for one token, and why that is not a typo

The prefill is a real chat completion with the real prompt prefix and `max_tokens: 1`, and
the one token is discarded. `max_tokens: 0` — the obvious way to ask a chat API to prefill
without answering — is **accepted and ignored** by these engines: the request comes back
having prefilled nothing, and the warm-up silently costs a request and saves nothing.
`usage.prompt_tokens_details.cached_tokens` is the receipt; it is published per turn in
`status.json`'s `turns.warm`, because a warm-up whose effect cannot be read from a socket
is a warm-up nobody will keep.

### What is measured, and what is only a budget

The pipeline's claim is testable and is tested: a fixture model that emits a partial every
400 ms and a fixture piper at 300 ms a sentence produce audio **before the last token**,
and the test was verified to fail with the piper slowed past the model (910 ms against
800 ms). The rolling-ASR test asserts the same shape for whisper: the tail slice is much
less than the whole transcription.

**Deployed and read on 2026-11-22, on services1.** Two bugs surfaced in the first ten minutes
(both above: a warm-up that refused its own turn, and a fast path that answered in silence),
and after them this is what the box reports for a two-tool model question asked as text —
`just voice-turn "what is the temperature in the workshop and is anyone likely around?"`:

| | buffered (measured 2026-11-16) | streamed (measured 2026-11-22, same box) |
| --- | --- | --- |
| first token | — (nothing leaves until the end) | 10 534 ms |
| **first audio** | ~15.8 s (13.3 s model + 2.5 s piper) | **11 317 ms** |
| answer complete | 13.3 s | 13 320 ms |
| speech generated | one block at the end | 19.7 s in 14 chunks — `VERDICT: … 1 999 ms before it was complete` |
| piper, per sentence | ~2 500 ms (2.4 s of it interpreter startup) | **178 ms**, `warm_sentences: 4, cold_sentences: 0, restarts: 0` |

*(What was here before the deploy was arithmetic on the buffered numbers, and it was
optimistic — which is what arithmetic is for: it said where to look, and the two things that
turned out broken were not in it.)*

So: **~4.5 s off the time a person waits to start hearing**, and the rest of the answer is
generated while the first sentence is being said. The `voice` object in `status.json` is what
proves the resident piper is the bigger half — 4 sentences warm, 0 cold, no restarts — and
`turns.last_first_audio_ms` is the single number to compare when anything downstream moves.

The remaining unread figure is the **audio** path: `asr_tail_ms` was 0 in the turns above
because they arrived as text. What a real `cam8` window costs with rolling transcription is
still an assumption until someone speaks to the satellite and reads it back
(`satellite-status.json`, and `/v1/turn`'s `asr_tail_ms`).

### The first deploy found the second bug the fixtures could not see

Same box, ten minutes later. `just voice-turn "is the space open"` returned a well-formed,
fast, **silent** turn:

```
     2 ms  log      10 tools: calculate, camera_activity, …
     8 ms  answer   Leigh Hackspace is CLOSED, since 17:44 UTC (6h 25m ago).
     8 ms  done     first audio 0 ms · 0 sentence(s) · 6 ms total
```

8 ms is the deterministic fast path doing its job (`engine: none (fast path)`), and it is the
reason the streaming pipeline never saw any text to speak: the fast path answers in one
`answer` event, with no `partial` events, and the new code fed piper **only** from the stream.
So the assistant's most common question in the room would have been answered with silence, and
every test stayed green because the fake model always streamed. The hand-back now lives in
`turn.rs` (no streamed text ⇒ the answer's text goes through the same splitter, same budget,
same worker), pinned by a test that fails with `["log", "answer", "done"]` and no audio if you
remove it — and `0 sentence(s)` in the `done` event is the number to look at when a turn is
quiet, because after that hand-back it can only mean *nothing speakable*.

### The one thing that reached production before it was true, and what the box said

The warm-up was switched on by default with the first deploy, and the two journals said:

```
slack-agent-listen: prefill: services2 (RTX 3060) cached 0/4079 prompt tokens in 4629 ms
voice-gateway:      error: another turn is in flight; this endpoint answers one at a time
```

That is the whole argument against it, in two lines: **4 629 ms of the 3060 to prefill a
prompt the real request was about to prefill anyway** (`cached 0` — the engine cannot reuse
what it has not seen), and because the warm-up holds the listen socket's one turn lock, the
question it was warming was **refused**. So three things changed, all of them now measured
rather than assumed:

- the warm-up fires **only while speech is still arriving** — never for a question that
  arrived as text, and `Timings::warm` says `skipped: the question was already final…`
  instead of pretending it tried (`turn.rs::warm_skip_reason`);
- `turn_gate::acquire_agent` waits up to 10 s for a **warm-up's** lock and counts it
  (`ask_waited_for_prefill`, `ask_wait_ms`), while two conversations still refuse each other
  at once — the documented refusal is about other people's questions, not about our own
  head-start;
- `infra.voiceAssistant.warmEngine` **defaults to false**, with the measurement in its
  option text, and `prefill_ever_hit` in the bot's `status.json` is the number that would
  justify turning it back on.

The streaming wins that survived are unaffected — rolling ASR, streamed tokens, the resident
piper — and `just voice-turn` still prints the timeline. What did not survive is the idea
that you can buy prompt-cache warmth by asking for it at the last possible moment.

### Things this deliberately did not do

- **The browser page still calls `/v1/ask`.** `/v1/turn` is ready for it and the contract
  is `events.rs`, but the page is a Vite/Solid build and changing it is an npm build, not a
  Rust edit. Open in `docs/follow-ups.md`.
- **No echo canceller appeared**, so the half-duplex rule is unchanged and barge-in is
  still blocked on the same thing it was blocked on (§"No echo cancellation").
- **Nothing new can be spoken to.** The camera's speaker is still off, and a streamed
  answer does not change the identity problem behind that.
- **One turn at a time remains true twice over**: the warm piper is behind a mutex, and
  `slack-agent listen` still 429s a second caller. Streaming makes a turn feel faster; it
  does not make the assistant a multi-way service, and the first wall a *fleet* of
  satellites hits is still that 429.

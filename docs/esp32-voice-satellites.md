# ESP32 and other dumb-microphone satellites: how to integrate with our voice assistant

**What this file owns.** Everything an agent needs to put a new audio endpoint — an ESP32,
ESP32-S3, a phone, a Raspberry Pi Zero, a browser tab it did not write — onto the hackspace
assistant without discovering the constraints by breaking them: the exact wire contract, what
exists versus what is only planned, where a device may sit on the network and what the router
will let it reach, the audio format every piece of this stack assumes, the numbers that say
whether a device is working, and the repo rules that bite specifically here.

**What it does not own.** Why the assistant is shaped like this, and the history:
`docs/voice-assistant-2026-11-16.md`. What each tool answers: `docs/services.md`. What `cam8`
is and its RTSP/audio facts: `cameras/README.md`. Deployment mechanics and the fleet's rules:
`AGENTS.md` (which you must read anyway).

**The one-sentence answer, before any of the detail.** **Be the browser, not the camera.**
`cam8` works because *we pull* its RTSP through go2rtc with `ffmpeg`
(`machines/services1/services/voice-assistant.nix`, `source`) — an architecture a small device
cannot serve. The browser page works because it *pushes* PCM to two HTTP endpoints over TLS,
and that path is deployed, load-tested through the front door, and speakable by anything with
a TCP client and a clock. A device that tries to copy the camera will find nothing listening
for it; a device that copies the page needs **no new server code at all**.

---

## 1. What is actually running (verify, then believe this)

Everything below was read out of the tree and the rendered units, not from a plan. Re-verify
with: `curl -fsS -m 10 https://voice.int.leighhack.org/status.json` and
`curl -fsS -m 5 http://127.0.0.1:8193/health` **on services1** (`AGENTS.md` rule 27 — check
`hostname` first; rule 21 — read the rendered unit rather than evaluating).

| Piece | Where | Address | Notes |
| --- | --- | --- | --- |
| `voice-gateway.service` | services1 | `127.0.0.1:8193`, published as `https://voice.int.leighhack.org` | loopback-only by its own check; no auth |
| `voice-wake.service` | services1 | `127.0.0.1:8195`, **only** through the gateway's `/v1/wake/*` | loopback-only; no auth; detection only |
| `voice-satellite.service` | services1 | listens on nothing; writes a status file the gateway serves | it is `cam8`'s ear, by pull |
| `slack-agent-listen.service` | services1 | `127.0.0.1:8194` | the answer path; **one turn at a time** |
| `hackspace-mcp` | services1 | `127.0.0.1:8191/mcp` | 10 tools; the answers come from here |
| whisper.cpp | aibox | `http://10.3.1.32:8082` | `ggml-medium` on **CPU** |
| piper | services1 | subprocess per request | `en_GB-alba-medium` → resampled to 16 kHz |

Code, in the order you will want it: `crates/voice-assistant/src/api.rs` (every route),
`crates/voice-assistant/src/agent.rs` (what an answer costs and when it is refused), `crates/voice-assistant/src/wav.rs` (the format),
`crates/voice-assistant/src/satellite.rs` (the reference *client*), `crates/voice-assistant/src/gate.rs` (the no-model VAD),
`crates/voice-assistant/src/wake.rs` (the address/strip rule), `crates/voice-wake/src/serve.rs` (the wake session
API), and the browser as a second reference client:
`crates/whisper-ws/web/app/src/state/assistant.ts` plus
`crates/whisper-ws/web/app/src/state/controller.ts`.

### The format, which is not negotiable

**16 kHz, mono, signed 16-bit little-endian PCM** (`wav.rs::SAMPLE_RATE`,
`gate.rs::SAMPLE_RATE`, `crates/voice-wake/src/serve.rs` requires 16 kHz per session). Every stage is built for it:
whisper.cpp wants it, piper's output is resampled *to* it, go2rtc already publishes
`L16/16000`, and the energy gate's frame maths is compiled against it. There is no resampling
in the gateway, so **the device resamples**. An ESP32 I2S MEMS mic at 44.1 kHz or 48 kHz will
be accepted (it is a WAV, whisper will cope) and will be wrong in a way nobody notices until a
threshold stops matching: the detector, the gate and every measured number in this area are
16 kHz figures.

---

## 2. The wire contract

`{ORIGIN}` = `https://voice.int.leighhack.org`. All bodies are capped at **900 KB** by nginx
(`client_max_body_size` in `voice-assistant.nix`, mirrored by the front-door row in
`common/front-door.nix`) and at **1 MiB** inside the app by `common_web::MAX_BODY_BYTES` — and
the two failures are *different*, which is the single most important sentence in this section:

> Over 1 MiB the app's `read_request` returns `None` and **the connection closes with no
> response at all**. A client reports a network error, not a 413. That is why the door refuses
> at 900 KB first: a 413 is a status a device can show somebody. 900 KB is ~28 s at 32 KB/s.

### 2.1 Wake (optional; class-2 devices only)

Exactly the Wyoming event model over HTTP, because a browser cannot open a TCP socket — and
therefore also the contract for anything else that cannot or should not run a model. The
session header is `x-voice-wake-session` (case-insensitive on the wire).

```text
POST /v1/wake/session   {"rate":16000}
     → {"session":"1","model":"hey_jarvis","rate":16000,"threshold":0.5,
        "cooldown":2,"max_body_bytes":1048576,"active_sessions":1}

POST /v1/wake/chunk     header x-voice-wake-session: 1
     body: raw s16le PCM, no header, even number of bytes
     → {"score":0.31,"peak":0.62,"detected":false,"audio_secs":4.50,
        "detections":0,"infer_ms":14.2,"cpu_ms_per_audio_s":31.7}

POST /v1/wake/reset     header x-voice-wake-session: 1     → {"ok":true}
DELETE /v1/wake/session header x-voice-wake-session: 1     → {"ok":true}
GET  /v1/wake/status.json                                  → engine counters + cost
```

Rules a device must implement, each of which is an error path that has been seen:

- **410 = the session is gone** (`--idle`, 120 s of silence, or an engine restart). Open
  another one; do not treat it as a transport fault.
- **400 on an odd byte count** — the body must be whole samples. If your DMA hands you 16040
  bytes, hold the last 8.
- **Chunk ≈ 0.5 s = 16 KB** is what the page sends (`WAKE_CHUNK_MS` in `assistant.ts`). A
  chunk that is larger works, but the engine scores **serially across all sessions** behind one
  `Mutex` (`crates/voice-wake/src/serve.rs`), so an oversized chunk is latency injected into everyone else's
  detection. One request in flight per device, and if it is not finished, drop audio rather
  than queue it — the page's rule is "back-pressure is a meter that stopped moving".
- **Cooldown is audio time, not wall clock.** A device that pauses streaming while the
  assistant talks does not get a fresh 2 s; a detection fired by the tail of its own
  conversation is the bug this prevents.
- **`POST /v1/wake/reset` after every turn**, or the model inherits the history of the
  sentence it just heard.
- `--max-sessions` is **16** and each session is its own ~7 MB model instance. A fleet of
  always-on streamers hits that ceiling, and the honest cost is ~31 streams **per core** at
  32.1 ms of CPU per audio-second (`crates/voice-wake/README.md`), which is a *shared* core
  doing everything else on that box.

### 2.2 Ask

```text
POST /v1/ask   {"text":"is the space open","voice":true}
     → {"answer":"…","engine":"strata","rounds":0,"tools":["space_status"],
        "agent_secs":0.04,"secs":2.51,"speech":"It's open…","audio":"<base64 WAV>",
        "audio_secs":3.10,"warnings":[]}
```

- `voice` defaults to **true**; send `"voice":false` to skip synthesis and save ~2.5 s.
- `audio` is a **base64 WAV, 16 kHz mono PCM16**, or `null` with the reason in `warnings`
  (which is also how "the model answered but piper failed" arrives — the answer is still good,
  so play nothing and say nothing rather than failing the turn).
- `audio_secs` is the thing to hold the microphone off for: **the half-duplex rule**
  (`crates/voice-assistant/src/satellite.rs`). Without echo cancellation, capture during playback is not information.
- `rounds: 0` means the deterministic fast path answered it (time, open/closed, a room
  temperature, extremes, devices, "what's on"). Those turns measured **2.5–3.4 s** end to end
  including synthesis; a model turn is **7.7 s** (one tool) to **15.3 s** (two tools) plus
  ~2.5 s of synthesis: ~13 s for a one-tool spoken answer. Budget your LEDs around that, not
  around a smart-speaker's sub-second expectation.
- **429 means the bot is busy.** `slack-agent listen` serialises turns on a lock and *refuses*
  a second caller rather than queueing it (`crates/slack-agent/src/listen.rs`); the gateway turns that into
  `"slack-agent is busy"` (`crates/voice-assistant/src/agent.rs`, which itself gives up at **150 s**). So **N satellites
  cannot ask concurrently**, and a device must back off and retry or lose the question. If you
  wire up more than a couple of devices, this — not CPU, not bandwidth — is the constraint you
  will hit first, and the fix is a queue with a stated policy (last one wins? first one wins?
  announce "one at a time"?). Do not silently retry forever: a room that keeps asking is a
  room that is not getting answers.

### 2.3 Transcribe / speak

```text
POST /v1/transcribe   body: a WAV file (≥ 44 bytes; ≥ 0.2 s of audio)
     → {"text":"…","audio_secs":3.02,"wall_ms":2710,"speech":true,"language":"en",
        "whisper_duration":2.87}

POST /v1/tts          {"text":"…","raw":false}   → audio/wav at 16 kHz + X-Voice-Secs header
GET  /v1/voice        {"voice":"en_GB-alba-medium","sample_rate":22050,
                       "speech":"audio/wav; 16000 Hz mono PCM16","length_scale":1.05}
```

`speech:false` from `/v1/transcribe` means whisper **described** the noise (`(soft music)` is
the real transcript from `cam8`'s mic) and it must never be treated as a request. Any caller
that asks the model about `(soft music)` is the reason `Action::Ignore` exists in
`crates/voice-assistant/src/satellite.rs`.

### 2.4 Things a device does not need

- **No CORS.** That policy (`--allow-origin`, exact match, no wildcard) exists for browsers.
  A device that is not a browser is unaffected, and must not ask for it to be relaxed.
- **No client certificate, token, or key.** There is no auth anywhere in this path — see §4.
- **No new vhost, DNS row or front-door row.** A device *dials* `voice.int.leighhack.org`; it
  is not *served* by anything. Adding a row for a satellite would create a second front door
  (`AGENTS.md` rules 14 and 24) and `machines/gw/int-dns.nix` would refuse the evaluation.

---

## 3. Where the ear lives: two classes, one rule

The tier is designed for exactly two device classes (`docs/voice-assistant-2026-11-16.md`):

| | class 1 — device detects | class 2 — dumb stream |
| --- | --- | --- |
| Examples | browser (wasm), an ESP32-S3 with PSRAM | `cam8`, a phone, a $5 board |
| On the wire | one flagged utterance | continuous 16 kHz PCM forever |
| Server cost | one whisper pass per *question* | one wake model per *device*, always |
| Needs | PSRAM + a model that fits | only a TCP client |

**The rule: exactly one detector per microphone.** A device that streams always *and* a client
that also listens is a double-count, and the failure is a wake word that fires twice, which
looks like a flaky model rather than two ears. In the page this is a single `<select>` with
three states (`off` / `this tab` / `server`) around one `MediaStream`, because calling
`getUserMedia` twice restarts the stream and a restarted stream drops samples exactly where a
wake phrase would be. A device gets the same discipline: one capture task, one gate, and a
setting that says which engine owns it.

**For an ESP32: detect on the device, if the chip is an S3 with PSRAM.** Reason, in order:
the server's sessions are capped and serialised (§2.1); an always-on 256 kbit/s stream per
device on 2.4 GHz is not free (§6); and battery or PoE-powered devices mostly want the radio
quiet. The server path is for the one or two devices that cannot do it, and for measurement.

---

## 4. Security: the reach *is* the boundary, so say what that means

There is no authentication anywhere on the voice path — not at the gateway, not at the wake
engine, not at the ask socket. Both binaries **refuse to bind anywhere but loopback** for that
reason, and the only supported way in from another machine is the front door's
`voice.int.leighhack.org` row, whose origin accepts only the router
(`CONFIG.ROUTER_ONLY`, `common/config.nix`), whose `*.int` ACL is
`CONFIG.LOCAL_NETWORK` (`allow 10.3.0.0/16`, `allow 192.168.2.0/24`), and whose `sso` is
deliberately off (a login in front of a voice request is a microphone that never gets to
speak).

Consequences to design against, not around:

1. **A device's identity is its source address.** Anyone who can route to `10.3.0.0/16` can ask
   the assistant anything the space's Slack channel can hear, at the cost of one LLM turn. A
   satellite should therefore get a **fixed reservation** in `common/dns-records.nix` (and if
   that ever becomes a machine in the flake, `common/hosts.nix` + `common/ssh-known-hosts.nix`
   are the same fact in three places, guarded by
   `machines/gw/lib/check-network-tables.nix` — read those three before picking an address).
2. **Never put a credential in firmware.** There is nothing for it to authenticate *to*, and a
   flashed ESP32 is a credential anyone with a USB cable can read. If a future endpoint needs a
   secret, it is a `sops.secrets.<name>` on the *server* (`docs/follow-ups.md` explains why a
   runtime-only secret must not join `secrets/secrets.yaml`), not a string in a device.
3. **This is a permanent microphone with an answer.** The voice gateway answers the same
   questions the bot answers in a public channel, and `/status.json` keeps the last 20
   question/answer pairs (`recordQuestions`, on by default, with `--no-record-questions` for a
   room that objects). Mounting a mic in a room where members talk privately is a human
   decision — the same decision `cam8`'s RTSP default credentials represent, which is still
   open.
4. **Do not "fix" the missing auth by exposing a port.** Widen nothing: an origin reachable
   from anywhere but the router is how a second front door gets built
   (`docs/router-front-door-migration.md`), and `X-WEBAUTH-USER`-style identity assertions have
   no place on this path.

---

## 5. Audio in, audio out: what an ESP32 actually needs to be

**This section is guidance from Espressif's and Home Assistant's documentation, and the pins
and claims must be re-verified against the ESP-IDF/ESPHome version you build with.** Nothing
here has been flashed in this fleet yet — say so in any write-up you produce, and do not let
"the doc says" become "it was measured".

### Chip and memory

- **microWakeWord needs an ESP32-S3 with PSRAM.** It is Kevin Ahrendt's TFLite-Micro port of
  the **same openWakeWord architecture** this network's browser page runs, and the phrase set
  includes `hey_jarvis` — the phrase this fleet already uses everywhere (ESPHome's own
  `esp32-s3-box-3.yaml` in `esphome/wake-word-voice-assistants` lists
  `models: [okay_nabu, hey_mycroft, hey_jarvis]`). So the wake word can be **literally the
  same model** on the device, in the browser, and — via `voice-wake` — on the server.
  That is the strongest argument in this file: one phrase, one architecture, three runtimes.
- **Espressif's own WakeNet (in `esp-sr`) is a different model set and I have *not* verified
  that `hey jarvis` is among the stock models** — the upstream tracker has an open *request*
  for a custom trained "Hey Jarvis" WakeNet10, which is evidence against it. Verify at build
  time (`idf.py menuconfig` → ADxF → ESP-SR → wake word) before choosing a phrase or a chip.
  WakeNet's advantage is MultiNet (offline *command* recognition) and a vendor-maintained
  pipeline; microWakeWord's advantage is that it matches our existing detectors.
- Model files are **data pinned by hash**, as everywhere else here: the browser's set is
  `crates/whisper-ws/package.nix` (`melspectrogram.onnx`, `embedding_model.onnx`,
  `hey_jarvis_v0.1.onnx`, `silero_vad.onnx`, plus `alexa`/`hey_mycroft`/`hey_rhasspy`/`timer`/
  `weather`), the server's is `wakeModel` in `machines/services1/services/voice-assistant.nix`
  (livekit `hey_jarvis.onnx`, `v2.1.3`, sha256-pinned), and a device's would be a third pin.
  **Record which phrase each one is and never imply they are the same file**: same phrase,
  different training, different scores. The first person to compare an openWakeWord score with
  a LiveKit score will conclude one of them is broken.

### Microphone, speaker, echo

- I2S MEMS mics (INMP441, ICS-43434) need **32-bit slots / 64 clocks per frame** and lie about
  their level in the ways that break our dBFS numbers (DC offset, and the
  `GND`/`VDD`-tied-low trap). Read samples as 32-bit and shift to 16-bit; then check the level
  (§7) — do not assume a "quiet" reading means a quiet room.
- A digital amp (MAX98357A class-D) into a real cone is **audible**, which `cam8`'s 20 mm piezo
  is not — that is the open question blocking the camera's speaker
  (`docs/follow-ups.md`: the software path is proven, audibility across a working room is not).
  An ESP32 with a cone answers that question by existing, and it is worth doing on the bench
  before anything is mounted.
- **Espressif's AFE (`esp-sr`) provides AEC, NS, VAD and WakeNet, with a playback *reference*
  channel** — its `input_format` string names channel roles, e.g. `"MMNR"` = two microphones,
  one unused, **one playback reference** — and its AEC supports **16 kHz only**, which is
  already our rate. Read that again in our terms: **an ESP32-S3 can do the echo cancellation
  this network does not have anywhere.** `cam8` reports `aec: false` and our only alternative
  is the half-duplex guard (drop the audio the assistant itself said). Barge-in — the thing in
  `docs/follow-ups.md` that "no AEC exists" is blocking — becomes possible *on the device*, and
  a device that can cancel its own output is a better satellite than the camera is, with no
  server-side work. If you build one, that is the feature worth measuring first, and the
  measurement is: play a known answer, then say a wake word, and count false self-detections.

### Framework

No precedent exists in this repo (`grep -ri esp32` returns only prose about future devices), so
this is a decision to make consciously rather than a house style to follow:

- **ESPHome** is the fastest route to a working device and gets Wi-Fi, captive portal, OTA,
  logging, a display, and `micro_wake_word` for free. Its `voice_assistant` component, however,
  expects **Home Assistant or a Wyoming server as the brain** — and we are neither (§8). So
  with ESPHome you keep its plumbing and do the turn yourself in a `custom` component.
- **ESP-IDF in C** is the most control and the only path to `esp-sr`'s AFE/AEC/MultiNet.
- **esp-rs (Rust)** is the house preference (`AGENTS.md` rule 1) and is genuinely viable for
  the application, but `esp-sr`, microWakeWord and the I2S drivers are C, so the useful parts
  arrive as bindings. Rule 1 exists because a second Python copy of a service drifted three
  times; an ESP32 firmware is not that risk, and if you can name the shared *logic* you would
  save by writing it in Rust, do it — if you cannot, C or YAML is not a rule violation, it is
  the honest tool. What must **not** happen is a firmware that reimplements our answer path:
  no rules about temperature or opening hours may live on a device (rule 2, and rule 31's
  warning about hand-copied facts a running process can answer — a device with a stale copy of
  the room list is a worse version of a bug we already fixed once).
- Neither ESPHome nor ESP-IDF is in this flake, and `platformio` fetches toolchains at build
  time over the network. **Do not try to build firmware into the system closure.** Keep the
  firmware in its own directory or repo, and keep in this tree only what this tree can own:
  the endpoints it dials, the address it holds, the pin and the hash of its model, and the
  document that says which version is deployed (rule 30's shape: firmware is documented with
  the firmware; this repo keeps the pins, the verbs and the network).

---

## 6. Where a device may sit, and what the router will do

From `common/vlans.nix` (the table dnsmasq, the firewall and the reservations all read):

| VLAN | Block | Purpose | A satellite here? |
| --- | --- | --- | --- |
| 225 | `10.3.1.0/24` | servers, wired LAN | yes if wired and on the bench; not a ceiling mount |
| 226 | `10.3.2.0/24` | Wi-Fi: cameras, lights, workshop wireless | **the natural home for a Wi-Fi satellite** — and the crowded one |
| 227 | `10.3.14.0/24` | workshop wired: Pi Room, printers, doors, aibox | best choice if there is a cable run |
| 222 | `10.3.22.0/24` | phones; **no dynamic range at all** | no — a device arriving there gets no lease |
| 1337 | `192.168.2.0/24` | the other network's traffic | not ours to place things on |

Two facts make any of 225/226/227 work without a firewall change:

- `machines/gw/firewall.nix`: `iifname "vlan*" tcp dport { 80, 443 } accept` — **every** VLAN
  can reach the front door.
- `CONFIG.LOCAL_NETWORK` allows `10.3.0.0/16`, so a device in any `10.3.x` block passes the
  `*.int` vhost ACL; and `*.int` names are answered from one `addn-hosts` file
  (`/etc/dnsmasq-int-vhosts.hosts`, generated from `common/int-dns.nix`) on every interface
  dnsmasq serves. Prove it from the device, not from here: resolve, then `curl -fsS -m 5
  https://voice.int.leighhack.org/health`.

**Bandwidth is the trap on VLAN 226.** Uncompressed 16 kHz mono PCM16 is **32 KB/s = 256
kbit/s**. `docs/camera-wifi-cells-2026-10-31.md` measured two identical cameras at identical
signal (−47 dBm) delivering **61 KB/s and 1.38 MB/s**, because of which cell each joined and
what else was on it — and 61 KB/s is barely twice what one always-on PCM stream needs. So:
detect on the device so the radio is idle until someone speaks; if it must stream, put it on a
5 GHz-capable S3 or on a cable; and when it misbehaves, **read the access point's retry
counters before blaming the firmware** (the same doc's rule: never change a device's
association over the very link you are changing).

Also true: `cam8`'s RTSP allows two clients and go2rtc already holds one. That is a camera
constraint, but it is the general lesson — **count the concurrent sessions your endpoint
allows**, because the failure (frigate starving when a second client appears) is invisible
until a reconnect.

---

## 7. Proving a device works, in the order that finds faults cheapest

1. **Is the gateway reachable at all, from that network?** `GET /health` through TLS. TLS on an
   ESP32 wants the CA bundle (`esp_crt_bundle_attach` / ESPHome's default): our certificate is
   Let's Encrypt, so `ISRG Root X1` must be in the bundle — and if the device fails TLS while
   `curl` succeeds, that is the difference, not the network.
2. **Is it *hearing*?** Report the mic's level in **dBFS** and compare against ours, because
   every number in this area is a dBFS figure and input gain is a measured property of a
   device, not a constant. For scale: `cam8`'s mic reads a mean of **−44.5 dBFS** in a working
   room and our energy gate needs `min_open_dbfs: -42` plus an 8 dB margin above the tracked
   floor (`gate.rs::Config`) — i.e. `cam8` sits *below* the gate's default opening point,
   which is why that follow-up exists. A device with a 20 dB quieter mic than a laptop does not
   have the same thresholds as a laptop.
3. **Did the detector fire, and how often?** `GET /v1/wake/status.json` gives `chunks`,
   `detections`, `audio_secs` and `cpu_ms_per_audio_s` for the whole engine. For false accepts
   per hour there is an offline harness that is the right tool: `voice-wake bench
   hey_jarvis.onnx <dir-of-wavs>` (peak score, detections/hour, step p50/p95) and
   `voice-wake oww-bench <model-dir> <dir> --gate off` for the browser's model on the same
   bytes. Both are on services1's PATH (`environment.systemPackages` in the module).
   **Thresholds are not portable.** `0.5` is upstream's default, and upstream's own figure is
   0.5–2.5 false accepts per hour of *conversational speech* — which is what a hackspace is,
   all day. The threshold for this room must be earned on this room's audio, and
   `docs/follow-ups.md` has that measurement open.
4. **Does a turn end to end?** `just voice-ask "is the space open"` (on services1), then
   compare its `rounds` and `secs` with the device's. If the device is slower by a constant,
   it is your chunking or your TLS handshake; if by a variable, it is contention with the
   single-turn ask lock.
5. **What is the room's floor?** `just voice-probe 8` **on services1** (it is build-host-guarded
   and says so), which prints the satellite's three numbers — `floor_dbfs`,
   `threshold_dbfs`, `peak_dbfs` — and distinguishes the two kinds of deaf: level too low, or
   gate shape wrong (and `voice-assistant gate-check <clip.wav>` for the column that says no).
   `just voice-status` renders the same numbers through the front door.

Recorded audio from a real device is worth more than anything else you can produce right now:
the wake comparison in `docs/voice-assistant-2026-11-16.md` is **blocked on exactly this** — a
person saying the phrase into two microphones, and one busy hour of room audio — because
piper-synthesised speech is out-of-distribution for one of the two engines and the room
recording on hand contains no wake word at all. A bench ESP32 recording WAVs to a host is the
cheapest way to unblock it: `just` recipes aside, `voice-wake bench` needs a directory of
16 kHz mono PCM16 WAVs and nothing else.

---

## 8. What does not exist. Do not write code against a plan.

- **No Wyoming TCP server.** `voice-wake serve` implements Wyoming's *event model* over HTTP
  and says why: a browser cannot open a TCP socket. Nothing in this fleet listens for raw
  Wyoming. `docs/voice-assistant-2026-11-16.md` describes Wyoming as the tier's protocol and
  says an ESP32 "can point at our ear with a URI" — that is the design note, **not the
  deployed contract**; the contract is §2. If you want ESPHome/HA satellites and Home
  Assistant's own voice pipeline to dial us, the missing piece is a small **Rust** Wyoming
  front end (length-prefixed JSONL + PCM over TCP, in `voice-wake`, loopback/VLAN-bound like
  the HTTP one). That is a real, bounded piece of work and it is the *only* way a third-party
  satellite speaks to us without custom firmware on its side.
- **No nixpkgs-wyoming shortcut.** `wyoming-satellite`, `wyoming-openwakeword`,
  `wyoming-piper` exist at this pin and were rejected once already: the satellite half needs
  ALSA (which `cam8` lacks), and its pipeline would be a **second answer path parallel to the
  bot** rather than a client of it. The second half of that reason still applies to any
  Home-Assistant-voice-pipeline shortcut: the answers must keep coming from `hackspace-mcp`'s
  ten tools, or two brains will disagree in public.
- **No inbound ingest for devices.** There is no "device POSTs a continuous stream and we cut
  it up" endpoint: the energy gate lives in our satellite process, which reads `cam8` via
  `ffmpeg`. A dumb device either uses `/v1/wake/*` (§2.1) or POSTs whole utterances to
  `/v1/transcribe`. If you want the other shape, it is new server code, and the first question
  is what it costs per concurrent stream.
- **No streaming TTS.** One whole base64 WAV per turn. ~2.5 s of synthesis is per-request
  process startup dominating (`crates/voice-assistant/src/tts.rs` reloads the voice each time), which is the biggest
  remaining latency on an otherwise-fast answer; the fix is written down in
  `docs/follow-ups.md` (a warm synthesizer, or `wyoming-piper` on loopback behind the
  gateway). A device with a buffer can hide part of this today: start playback as soon as
  enough bytes have arrived *if* you can compute the WAV length honestly, and never pretend
  the rest is coming.
- **No device fleet management.** No config channel, no OTA path, no per-device version
  reporting, no MQTT contract for satellites (there is an MQTT broker — `mqtt.int`, see
  `docs/mqtt-int-legacy-answer-2026-11-02.md` for how that name resolves and why it must not
  be answered by the front door — but no satellite protocol on it). If a device publishes
  status, say what topic and who reads it, and remember that a `*.int` name a *socket* is
  dialled through is a third class of name that neither drift guard can see (rule 24).
- **The camera's speaker is off by policy, not by omission** (`speaker = "off"` is the default
  option, with the two conditions in `voice-assistant.nix`'s header). A device that *can* be
  heard does not inherit permission to be used: same decision, new hardware, and someone has
  to have heard it in the room first.

---

## 9. Rules of this repo that bite specifically here

- **Read the rendered unit** after touching any flag (`AGENTS.md` rule 10). The wake-word list
  is a quoted `ExecStart` argument because it contains phrases with spaces, and unquoted it
  silently becomes the single wake word `hey`.
- **A silent zero is a bug** (the lesson applied three times in one day here: a score of
  `0.000` that meant the pipeline never ran, a cost line of `0.0 ms` printed by
  `Instant::now().elapsed()`, and a `peak_dbfs` whose derived default read "0 dBFS" — full
  scale — in a quiet room). When a device reports nothing, make it report *why* nothing, with
  the numbers that distinguish the two causes.
- **Options that are accepted and ignored with a 200** are the failure class that has bitten
  twice (`enable_thinking`'s wrong spelling; an older MCP server ignoring a new argument). If
  you add a flag to a device or to the gateway, prove the deployed binary changes behaviour,
  not that it starts.
- `just check` before deploying, `just check-build` when anything is built, and the GOLDEN
  RULE confirm in the same shell invocation for any switch (`AGENTS.md` rule 4). Nothing a
  satellite needs should change a machine's behaviour while people are in the room, so
  prefer `just check-eval` + a bench build on services1 until a decision is earned.
- **`just check` validates this file.** `common/check-doc-pointers.nix` checks the recipe
  names, repo paths and doc links `AGENTS.md` and these docs promise — so a path added here
  must exist, and `just check-eval` is the gate that says so.

## 10. The first build, if you want the shortest path to a true statement

1. Bench only. **ESP32-S3 with PSRAM + an I2S MEMS mic + a MAX98357A and a real cone.** Not
   mounted, not in the room, not trusted.
2. Wi-Fi on VLAN 226 with a reservation, and prove `GET /health` over TLS from that address.
3. Report the mic's dBFS floor and peak for one minute of a working room. Compare with
   `cam8`'s −44.5 dBFS and the gate's defaults. This one number decides whether the device is
   better than the camera as an ear, before any model is involved.
4. `hey_jarvis` via microWakeWord on-device (verify the phrase ships), threshold at the
   upstream default, and **record**: detections per hour in a busy room, and recall from a
   person saying it mid-sentence. Isolated-phrase recall is not a result; the only isolated
   figure in this repo's own history is a *warning* — piper speaking "hey jarvis" scored 0.063
   on the server's engine and 0.000 on openWakeWord's.
5. On detection, send the flagged window to `/v1/transcribe`, then `/v1/ask` with
   `"voice":true`; decode base64 to the DAC; hold the mic off for `audio_secs` plus a guard;
   count what it would have heard and thrown away.
6. Then the interesting experiment nobody can do on `cam8`: turn on the AFE's AEC with a
   playback reference and measure self-detections with the speaker running. If that works, the
   ESP32 is not a cheaper camera — it is a better satellite than anything we have, and
   barge-in stops being blocked on server-side work.

Every step above produces a number that a sentence in this file or in
`docs/follow-ups.md` has been waiting for. That is the point of the device.

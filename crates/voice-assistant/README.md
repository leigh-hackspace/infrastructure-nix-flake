# voice-assistant

A voice assistant for the space: **audio in from a camera or a browser, text to the
bot that already answers in Slack, speech back out.** One binary, five verbs, no
secrets, and — deliberately — nothing playing out loud until someone has decided a
ceiling speaker should talk over a room.

```
  cam8 mic ──ffmpeg──▶ voice-satellite ─┐  /v1/voice (WAV, multipart)
  browser mic ─────────────────────────┘
                    voice-assistant serve  :8193  (loopback only)
                      ├ POST /v1/transcribe → whisper.cpp (aibox :8082)
                      ├ POST /v1/ask        → slack-agent listen (:8194)
                      ├ POST /v1/tts        → piper            → WAV
                      └ GET /health /status.json
  speaker off by default: the answer's WAV is written to the state directory.
```

## The device boundary: `source`, `sink`, `drivers/`

The turn logic — gate, wake-word rule, half-duplex arithmetic, the two HTTP calls — knows
nothing about hardware. It drives two traits:

| trait | question it answers | implemented by |
| --- | --- | --- |
| `crate::source::Source` | hand me 16 kHz mono s16le frames, and tell me when they stop | `drivers/ffmpeg_stream.rs` (the network's restream today; a file or a pipe work unchanged) |
| `crate::sink::Sink` | did that reach a room, for how long, and is your output cancelled out of the mic | `drivers/play_nothing.rs` (the deployed default), `drivers/rac_ssh.rs` |

Two rules that look like device details and are **not**, because they are the two pieces of
judgement a platform must inherit instead of re-implementing: **when to stop believing what it
hears** (`capture_to_discard`, driven by what the sink reports rather than by a hardcoded belief
about a camera) and **whether a transcript was addressed to us** (`wake.rs`, `gate.rs`). Put
either in a driver and every new device re-implements it — or forgets it, which is the same bug
with a longer fuse.

`common/check-platform-boundary.nix` (run by `just check-eval`, and as the flake check
`platform-boundary`) asserts the core holds no host, URL, vendor name or `Command::new`, **and**
that the drivers still hold them: a guard satisfiable by deleting the thing it forbids is a
guard nobody trusts. Its header states the one way it can be lied to (flag names — `--ffmpeg`,
`--ssh-key` are the deployed `ExecStart` contract, so they stay).

What a new platform is supposed to cost: one module in `drivers/`, one line in `open_sink` or the
source factory, and a row in the unit. If it needs anything from the core, that is a finding
about the boundary. `docs/esp32-voice-satellites.md` is the contract for the next one.

## Verbs

| | |
| --- | --- |
| `serve` | the gateway. **Refuses to bind anything but loopback** (see *Reach*). |
| `satellite` | the camera end: ffmpeg in, gate, wake decision, ask, speak. |
| `probe --seconds N --out F.wav` | the same path **minus the assistant**: capture, gate, transcribe, stop. `--seconds 0` listens until someone presses Ctrl-C. The measurement tool for the one question you cannot answer at a desk — how often the space's music looks like speech — and `--out` leaves a WAV so a human can listen to what the camera heard. When the gate never opened, that file is the **whole capture**, because "why did it ignore me" is answered by hearing the room. |
| `gate-check clip.wav [--calibrate]` | the gate alone, frame by frame: level, floor, threshold, the three shape numbers, and *which one said no*. See below — this is the verb that found the bug below. |
| `ask "…"` | text in, answer + spoken text + speech WAV out. No microphone involved. |
| `tts "…" -o out.wav` | piper directly. `--say` is not implemented: this writes a file, it does not play one. |
| `gate-check <file.wav>` | the speech gate alone, frame by frame, with a `--calibrate` pass that prints a `--gate-base` worth starting from. For when the gate is wrong and you need to know *which* side. |
| `just voice-turn "…"` | not a verb of this binary: the gateway's `POST /v1/turn` from a shell, **streamed** — every event with the millisecond it arrived, a VERDICT line about whether audio began before the answer ended, and the speech written to `/tmp/voice-turn.wav`. This is the verb that measures the section below. |

## The gate, and why energy alone could not be trusted here

The camera's mic is open. It hears the space's stereo, tools, and about ten people's
voices, and every chunk of that which we send to whisper costs two seconds of CPU on
aibox — worse, whisper *invents* words for hums and stings (`(soft music)` at least
admits itself; earlier takes transcribed "Okay so this is where you plug in the…" from
an unrelated sound).

Energy-only VAD failed in a direction nobody sees coming: it fired on **music** as well
as speech, and its "floor" rose to meet the music and swallowed the speech under it.
The floor must track the *quiet*, not the average, or a room with a stereo never has a
speech side of the line at all. Three things, at frame granularity (40 ms):

1. **`level > floor × gate_mult`, floor = an 85th-percentile-ish estimate of the quiet**
   — an asymmetric EWMA (rises 8× faster than it falls), so a pause between sentences is
   judged against the room, not against the sentence that just ended.
2. **Syllable modulation.** Speech has 2–12 Hz energy in its own loudness envelope;
   a sustained tone or a steady hiss does not. Measured at the gate's own timescale:
   sustained 440 Hz → 0.05, a spoken sentence → 0.36, a 1 s tone with tremolo → 0.99.
3. **Spectral shape**, two numbers that survive being heard through a stereo mix:
   `hf` = energy above ~1 kHz (speech has fricatives; hums and engine drones mostly
   do not), `voiced` = the low-frequency share (music and machinery have a lot of it, so
   this is a *negative* indicator).

That is a filter, not a classifier. It will still open for a hand clap, and the
`gate_open`/`heard` counters in `status.json` exist so you can watch what it is doing in
the real room and set `--gate-base`/`--gate-mult` from what you see rather than from a
guess. `just voice-probe` is the verb that tells you.

**The factor of two in the Goertzel** (`gate.rs`): the classic form returns `|X_k|²`
after the recurrence, not `|X_k|`. Reading it as a magnitude made the modulation of
*everything* read 0.15–0.3 — a perfectly good feature computed wrongly, invisible until
a unit test asserted the obvious thing about it. That test is the reason the constant
is written once, as `scale`.

## Wake word, and why it is on the transcript

Wake-word decisions want to happen *before* transcription because transcription is the
expensive part. Here the expensive part is already paid: the page runs
[OpenWakeWord](https://github.com/dscripka/openWakeWord) in the browser
(`crates/whisper-ws/web/app`) against the same 16 kHz PCM, and the satellite would have
to run its own model on services1. So v0 decides on the **transcript**
(`wake.rs::decide`), which has three properties worth knowing:

* it **cannot distinguish "hey Jarvis" from somebody saying the word in a sentence** —
  hence `must_be_addressed` and the whole-word matching (a `jar` inside "camera" is not
  a wake word, and neither is `space` inside "namespace": short names match only as
  whole words, because the space's own vocabulary is full of longer words that contain
  them);
* the wake word has to be *said*, and the gate has to have heard it — if it clipped the
  first syllable, the transcript says "arvis" and the request is dropped. Look at
  `last_transcript` in `status.json` when "it ignores me";
* **whisper does not transcribe "hey jarvis" as "hey jarvis"**: the measured rendering of a
  piper-spoken "hey jarvis is the hackspace open right now" through this exact path is
  `"K Jarvis is the Hackspace open right now."`. The bare `jarvis` in the default list is
  what saves it — a list of only two-word phrases would be deaf to the phrase it is
  listening for, which is the kind of thing no unit test can see because both sides of the
  test are spelling.
* **`--wake-required` defaults true for the camera** (a room must get to ignore it) and
  the page passes the wake flag explicitly, so a push-to-talk press is an address.
  Anyone with a microphone can already make it answer; `--wake-required=false` makes it
  answer to anyone with a voice, and that is the line, not this flag.

## The gate's own numbers, or "why is it not hearing me" has two answers

`probe` and `satellite` print a five-second heartbeat — `floor`, `threshold`, `loudest`,
speech-frame count, window count, ffmpeg restarts — because the two ways a room gets ignored
need **opposite** fixes and a log of final decisions cannot tell them apart:

* loudest frame *below* threshold → quiet room, or a threshold above the room: turn
  `--min-open-dbfs`/`--margin-db` down;
* loudest frame *above* threshold and still no window → the shape tests or the
  run-of-`--open-frames`: run `gate-check` on the clip and read the column that says no.

Measured on `cam8`'s mic at 00:28, an empty space: **floor −48 dBFS, threshold −40, loudest
frame −43** — correctly a quiet room. The same gate on a piper-spoken question at the same
defaults: **median −20, speech frames 62 of 135**, opens at 0.38 s. So the two ends of the
range are measured on this hardware, not assumed.

**The bug `gate-check` exists for, which it found on its own first use:** a window only ends
after `hold_secs` of trailing silence, so **whatever the gate was holding when the input
stopped was thrown away** — the end of a test clip, yes, but also go2rtc dropping its
session, a `--seconds 8` probe running out, and any ffmpeg restart. It presented as "the
gate never opened" on a clip whose trace showed it open at 0.38 s. `Gate::flush` now hands
that window to the caller at end-of-input (`flushed: true`, logged as *"the stream ended
mid-word, sending it anyway"*), and `--min-secs` still filters it: a clipped last syllable
beats no transcript.

The same first run also caught the diagnostic lying: `floor`/`threshold` were only refreshed
on a completed window or a five-second timer, so a short stream reported their **initial**
`0.0` — i.e. "0 dBFS", full scale, "something is clipping", about a silent room. Refreshed
per chunk now, and the peak is read from the gate (`peak_frame_dbfs`) so the heartbeat and
`gate-check`'s table are one measurement, not two that differ by 8 dB.

## No echo cancellation, so the answer is a mute button

`cam8`'s audio has `aec: false`, `ns: false`, `agc: false` (`rac status`). It has no
echo canceller and neither do we, so if it listened while it spoke it would interrupt
itself — and worse, its own voice would be transcribed into a new question. Hence
half-duplex, as a rule with teeth in `satellite.rs`: audio captured while an answer is
in flight is **dropped** (`dropped` in `status.json` counts it) and the gate is reset
when playback ends. If a real speaker is fitted and someone wants barge-in, the way to
have it is server-side AEC against the *playback clock* — which means knowing the
end-to-end latency, which `audio_ms` and `played_at` in the status file record on
purpose.

## Reach: loopback, or anyone's browser

The gateway binds loopback and `serve` **refuses any other address** — the flag is for
testing. It is not authentication: it has none. On the LAN it is reachable through the
front door (`voice.int.leighhack.org`, `machines/services1/services/voice-assistant.nix`),
and CORS is the one place a browser can be turned away: `--allow-origin` echoes back
only an exact match, and `*` would mean any page a member opens can ask the space's
questions and read the answers. A phone reaches the page on
`whisper.int.leighhack.org`, which is the only origin allowed by default.

**The size ceiling, and who refuses first.** Every server in this fleet shares
`common_web::MAX_BODY_BYTES = 1 MiB`, and over that limit `read_request` does not answer
413 — it returns `None`, so the socket closes with **no response** and a browser reports
"cannot reach the assistant" for a recording that was merely long. The front door therefore
refuses at **900k** (`machines/services1/services/voice-assistant.nix`), which is ~28 s of
16 kHz mono speech and always beats the app to the answer, so the failure a member sees is a
status code. Raising the real ceiling means editing a constant four other servers share, so
don't: raise it deliberately or make the page stop at 15 s, which it does.

**The trap this creates:** the page and the API are different origins, so a request that
works with `curl` can still fail in a browser with a CORS error, and the symptom is a
red banner on the page, not anything in the gateway's log. `just voice-status` proves
the gateway is fine; the browser's network tab names the missing origin.

## Latency is a design decision, not a tuning problem

Measured on 2026-11-16, these units' own binaries: 3 s of mic audio → transcript
**2.7 s** (whisper `medium` **on CPU** — `whisper-server` holds no `/dev/dri` fd,
strata owns the iGPU); one-tool question → answer **7.7 s**; two-tool **15.3 s**;
31 words → 7.8 s of speech in **2.5 s** (piper, per-request process). A spoken turn
end-to-end is **~13 s** for a one-tool question and minutes for a chatty model.

Which means: the budget belongs to the **model**, not to whisper or piper. The two
things that actually shorten a turn are already in the tree — the deterministic
calendar path (`triage::events_window`: one HTTP GET, no model) and the spoken budget
(`agent::MAX_SPEECH_TOKENS = 200` vs the Slack 700, and a spoken system prompt in
`prompt.rs::spoken_system_prompt` that forbids bullets, URLs and emoji and says
*start with the fact*). The `ask_budget` test that fails if `MAX_ANSWER_TOKENS` is
lowered below the speech budget is the guard against the version where the voice path
silently inherits Slack's verbosity. `spoken.rs::from_answer` is the backstop for when
the model ignores its prompt anyway, because it has.

## The turn is a pipeline: four stages, two threads, one clock that matters

The numbers above are honest and their **sum is a lie**, because they were measured one
after another and the old code ran them one after another: 2.7 s whisper → 7.7 s model →
2.5 s piper = ~13 s of a room standing still. Each stage was finished before the next
started, and every one of those waits is a choice.

So `turn.rs::run` overlaps them, and the only number a person in the room experiences is
**the first sample of audio**:

```text
  audio in ─▶ rolling ASR ─▶ engine warm-up ─▶ model stream ─▶ sentence splitter ─▶ piper ─▶ pcm out
                 │                │                │                  │                      │
                 └────────────────┴────────────────┴──────────────────┴──────────────────────┴─▶ events
```

- **`asr.rs`** transcribes an open window *as it fills*, in slices with overlap, and joins
  the seams. What the speaker waits for at the end is one slice (the tail), not the length
  of their sentence. `Rolling`'s seam-deduplication exists because whisper will happily
  transcribe "the space is" / "is open" as "the space is" / "is open" — or as "the space is
  the space is open", and both spellings are wrong in different ways.
- **`gateway.rs`** (satellite side) and `run` can fire a **prefill**: a `max_tokens: 1` chat
  call with the real prompt prefix, so the engine is holding this conversation before the real
  request arrives. `max_tokens: 0` is ignored by these engines (measured; see
  `crates/slack-agent/src/engine.rs`), so it is `1` and the one token is thrown away.
  **It is off by default, because the box told us to stop** — see the warm-up rule below.
- **`agent.rs::ask_stream`** reads the model's SSE and hands over each `partial` as it
  lands. The tool-call fragments that arrive keyed by `index` are reassembled in
  `slack-agent`, not here.
- **`sentence.rs`** decides what is speakable yet. Its rules are the ones a listener would
  notice: `12.2` and `e.g.` and `J. R. Smith` are not sentence ends; a completed unit
  shorter than a breath waits for company; a run-on answer with no full stop is cut at a
  clause boundary near its limit rather than never. **A full stop at the very end of what
  has arrived so far is not a sentence end** — the next token decides whether it was `e.g.`
  or the end of the answer, which is why the splitter holds it for one character.
- **`tts.rs`** speaks each sentence through a **warm** piper (`--output-dir` mode: one
  process, one WAV per line, the path announced on stderr), and falls back to a cold
  one-shot piper if the resident one dies — with the fallback *named in the timings*,
  because 2.4 s of silent startup per sentence would otherwise look like a slow model.

Two threads, and no more: the turn's thread drives ASR and the model socket and is the
**only** writer to the response; the voice thread owns the piper process. They talk over
two `mpsc` channels, which is what keeps the splitter and the process single-threaded —
each holds state that sharing would corrupt (a half-assembled sentence, a live child).
After **every** model event the voice queue is drained, and that one line is the entire
mechanism by which audio reaches the room while the model is still answering.

`Out` (`events.rs`) **is** the wire contract: `asr`, `log`, `token`, `reset`, `audio`,
`warn`, `answer`, `done`, `error`, defined once with `to_json`/`from_json` beside it, so
the gateway, the satellite and the page cannot have three ideas about what `audio` means.
`from_json` ignores events it does not recognise, so an old satellite on a new gateway
keeps playing audio instead of failing to parse the turn. `turn_routes.rs` is the HTTP
surface (`POST /v1/turn`, `POST /v1/warm`): `web::Sse` writes one event per flush, sets
`TCP_NODELAY` and `X-Accel-Buffering: no` — without that last header the front door sits
on the whole answer and the pipeline's entire point arrives as one slow 200.

**The client leaving is a normal ending.** `emit` returns `false` and everything upstream
stops — no more whisper slices, no more model tokens, no more sentences to piper. A turn
cancelled by a closed socket must not be logged as an error, or a page someone navigated
away from reads as an outage.

### A streamed answer is spoken sentence by sentence — so what speaks an answer that streamed nothing?

The deterministic fast path (`space_status`, the clock, a room temperature) answers with one
`answer` event and `first_token_ms == 0`: no `partial` events at all, because no model ran. A
pipeline that feeds piper only from the stream would answer *"is the space open"* — the most
common question in the room — with a **silent** well-formed 200. That is exactly what the first
deployed version did (`answer` at 8 ms, `0 sentence(s)`, no audio), found by running the verb
against the live box, which is the kind of fault no unit test of a fake model will show you
unless someone writes the fake to behave like the fast path. So: if no text ever streamed, the
`answer` event's text goes through the **same** splitter, the same `spoken::speakable` budget
and the same worker, and `sentences == 0` in the timings afterwards then means the answer
genuinely had nothing speakable in it rather than that this code forgot.
`turn::tests::a_fast_path_answer_with_no_streamed_text_is_still_spoken` is the falsifiable
version (with the hand-back removed it fails with `["log", "answer", "done"]` and zero audio).

### The warm-up rule: fire it while someone is still talking, or do not fire it

Deployed and switched on once, the warm-up produced this line in the bot's own journal:

```
prefill: services2 (RTX 3060) cached 0/4079 prompt tokens in 4629 ms
```

and in the gateway's, four seconds later:

```
error: another turn is in flight; this endpoint answers one at a time
```

Two facts, both worth keeping. A warm-up of the spoken prompt is **4.6 s of the GPU and
reuses nothing the first time**, because the engine has to prefill the prefix either way — so
fired the instant the question is final, it is the same work done twice with the second copy
in front of the queue. And `/v1/ask/stream` and `/v1/ask/prefill` share the listen socket's
one turn lock, so it did not merely delay the turn, it **failed** it.

The rule therefore lives in one function, `warm_skip_reason`, and says so in
`Timings::warm`: fire **only while speech is still arriving** (an ASR partial with slices
left, which is also what the satellite does through `/v1/warm`); never for a question that
arrived as text; and report `skipped: the question was already final…` rather than pretending
the attempt was made. On the bot's side, `turn_gate::acquire_agent` waits up to
`PREFILL_WAIT` for a **warm-up's** lock — with the counters `ask_waited_for_prefill` and
`ask_wait_ms` to show how often — while two conversations still refuse each other
immediately, as documented. `prefill_ever_hit` in `/status.json` is the one number that would
justify switching the feature on: a fleet of hits means prefills are being reused, and the
seconds they buy arrive before anyone has stopped talking.

A test that cannot express this is a test that will ship it: the pipeline's fake
`slack-agent` answers `/v1/ask/prefill` and `/v1/ask/stream` on independent sockets, so the
whole failure class passed green and appeared only on services1. `turn_gate.rs`'s own test
models the shared lock instead.

### What is measured, and what is not

`turn::tests::the_first_audio_arrives_while_the_model_is_still_answering` is the test to
read before changing any of this. The fixture answers a partial every 400 ms (a decode at
the measured 14–39 tok/s) and piper takes 300 ms a sentence; the assertion is that the
first audio is earlier than the **last token**. It was verified to fail — piper slowed to
900 ms, first audio 910 ms against a last token at 800 ms — which is the only reason a
timing test is worth having. The assertion is *not* against the `answer` event:
`Out::Answer` is emitted only after the voice queue has drained, so a test aimed at it
would pass for a pipeline that buffered everything. `just voice-turn "is the space open"`
prints the same timeline against the live gateway, with a VERDICT line.

**Not yet measured: any of it on the network.** The 13 s figure at the top of this file is
the buffered path's, and the numbers above are a *budget*, not a result. What the room
actually waits for now is `first_audio_ms` in `status.json`'s `turns` object, and the
honest statement is that it has never been read off a deployed box.

## piper is a process, and that is the cost

`/run/current-system/sw/bin/piper` does not exist: piper is a Python wheel, so the unit
points `--piper-bin` at `${pkgs.piper-tts}/bin/piper`, a real binary in the store. It
starts the interpreter and loads the ONNX model **per request** (~2.4 s of the 2.5 s
above is startup), which is why a sentence-at-a-time voice would have been *slower*
without this section: five sentences at 2.4 s of startup each is 12 s of Python
interpreter for a two-second answer.

So `Synthesiser` keeps one **resident** piper in `--output-dir` mode — one process, one
WAV per line, the path announced on stderr, read and deleted — and falls back to the
one-shot call when the resident process dies, counting the fallback in `TtsStats` and
publishing `warm: true|false` in `status.json`. Two things this buys and one it does not:
the 2.4 s is paid once per gateway, per voice; a sentence's audio lands ~0.1–0.3 s behind
the text (a 60 MB ONNX model on a box that also runs postgres); and `warm` in the status
file is a **live** question — "is the process up right now" — with the counters beside it
as the history, because "will the next answer be quick" and "has this been working" are
different questions and operators ask both.

Voices are data: nixpkgs packages the engine and **no voices**, so the deployment pins
the model + its `.onnx.json` in a `linkFarm` (`machines/services1/services/voice-assistant.nix`).
piper refuses to load a voice without the JSON beside it (`FileNotFoundError`), so the
two files are one store path or neither.

## Output to the camera

`satellite.rs` can push audio with `ssh root@cam8 rac play - -r 16000` — measured
working (`rac: played 0.3s, 5600 samples`), **audibility unconfirmed**, and the default
is `--speaker off`. The reason to keep it off is not the sound level, it is the
identity: that ssh needs the fleet's hop key, which sits in operator accounts and which
a `DynamicUser` must not read. The two honest ways to have it (a dedicated
`voice-speaker` account whose *public* key is installed on the camera, or a
systemd-managed agent holding the hop key) are in `docs/follow-ups.md`. Until one of
them is chosen, a camera that listens and a page that speaks is the useful half.

The RTSP alternative (a `SETUP … transport=udp` backchannel, or ONVIF Profile T
`SetAudioPlayerConfiguration`) is unexplored; `/var/www/config-ha.html` on the camera
suggests Home Assistant discovery would treat it as a **microphone** (`platform: hass,
type: mic`, 16 kHz fltp) — the same answer as everything else so far: it is an input,
and getting audio *out* of it is the unsolved direction.

## Tests worth running before you change anything

`cargo test -p voice-assistant` covers the four places where a wrong unit was
found by a unit test rather than by the room: the Goertzel scale, the gate's
behaviour on a tone vs a speech-shaped envelope, `b64`'s padding (a missing pad
corrupts every 4th byte of a WAV, and the first symptom is a click in the answer), and
`wav`'s `fmt` offset (`0:16` vs `0:12` is "the file is 44 bytes shorter than it is").
Plus the two rules that are really policy: `serve` refuses non-loopback binds, and the
spoken token budget never exceeds the Slack one.

The streaming turn added four more, and they are the ones to read first if you are about
to touch the pipeline, because each pins a claim that a plausible refactor breaks:

| Test | The claim |
| --- | --- |
| `turn::tests::the_first_audio_arrives_while_the_model_is_still_answering` | audio begins **before the last token**. Verified to fail when piper is slowed past the model (910 ms against 800 ms) — see the section above. |
| `turn::tests::the_room_pays_for_the_last_slice_not_the_whole_transcription` | `asr_tail_ms` is much less than `asr_ms`, which is the whole case for slicing audio at all. |
| `turn::tests::a_caller_that_hangs_up_ends_the_turn_without_an_error` | a closed socket is a clean ending, not a fault. |
| `events::tests::every_event_round_trips_through_the_wire_format` | every variant survives `to_json`→`from_json`. It has already earned its keep: the first version of this line silently read a `prompt_tokens` key that nothing wrote, so the cache hit rate arrived as zero forever. |

They run against fixtures in `testkit.rs` — a fake piper (both `--output-raw` and
`--output-dir`), a fake `slack-agent` that replays a scripted SSE body with sleeps, a fake
whisper. The sleeps are the point: a fixture that answered instantly would let an ordering
test pass for the wrong reason, and every timing assertion would be theatre. `testkit`
speaks only the wire, never the internals, for the same reason `just` recipes curl through
the front door instead of calling functions.

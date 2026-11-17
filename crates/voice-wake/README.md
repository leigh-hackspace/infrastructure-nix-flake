# `voice-wake` — the always-on ear, measured before it is trusted

Wake-word **detection**, separated from transcription, so that ASR runs on the turns
someone aimed at the assistant instead of on every sentence the room happens to say.

The design and the argument for it are in `docs/voice-assistant-2026-11-16.md`. This file is
the measurements, because an always-on detector in a shared space is judged by how often it
interrupts strangers and that number cannot be inherited from a vendor's README.

## What it is today: a harness, not a daemon

`src/main.rs` scores WAV files with a streaming model and prints the three numbers that
actually decide a deployment — peak score, detections per hour of real audio, and
milliseconds of CPU per second of audio — so the next step (a Wyoming wake server) can be
sized and thresholded from data instead of hope.

```
nix develop --command bash -c 'cd crates/voice-wake && cargo build --release'
./target/release/voice-wake bench /path/to/hey_jarvis.onnx room.wav --threshold 0.5
./target/release/voice-wake score /path/to/hey_jarvis.onnx clip.wav        # a line per step
```

Inputs are mono 16-bit PCM WAVs — what `voice-assistant probe` writes and what the camera's
stream carries. `--chunk` is there so the CPU figure can be interrogated: it defaults to
1280 samples, one 80 ms step, which is what a network device actually delivers.

The engine is [`livekit-wakeword`](https://github.com/livekit/livekit-wakeword) 0.2.0
(Apache-2.0): one streaming `.onnx` per phrase, 80 ms steps, per-model state owned by the
crate, resampling and the melspectrogram/embedding frontend included. Nothing here is a
model. `ldd` on the release binary is `libc`/`libm`/`libgcc` only — the crate resolves to the
**pure-Rust `tract`** backend, so there is no `libonnxruntime` blob to fetch at build time,
which is what makes this buildable inside the sandbox at all (verified: `nix build
.#voice-wake` succeeds, and the resulting binary scores the same 0.947 on the fixture).
`crates/voice-wake/package.nix` records what pins its 147 dependencies — and that the
`cargoHash` you would expect to do it does not.

## Measured, 2026-11-17, on services1, one thread (`RAYON_NUM_THREADS=1 OMP_NUM_THREADS=1`)

First, proof that the harness can pass — LiveKit's own fixtures, which are labelled:

| clip | expected | peak score |
| --- | --- | --- |
| `client/python/tests/fixtures/positive.wav` | contains its wake word | **0.947** |
| `client/python/tests/fixtures/negative.wav` | contains none | **0.015** |

Then the numbers that matter:

| what | result |
| --- | --- |
| CPU, 240 s of real room audio, one step per call, **`cargo build --release`** | **38.3 ms per second of audio = 3.8 % of one core ≈ 26 streams/core** |
| same harness, **`nix build .#voice-wake`** | **32.1 ms/s = 3.2 % of one core ≈ 31 streams/core** |
| same audio, 16000-sample calls | **38.1 ms/s** — identical, so the cost is inference, not per-call overhead |
| ≈ concurrent streams per core, single-threaded | **~26** (≈300 on aibox's 12 cores, before headroom is spent on anything else) |
| step latency | p50 2.95 ms, p95 4.52 ms, max 9.89 ms for an 80 ms step |
| 240 s of midday room audio, threshold 0.5 | peak **0.031**, **0 detections** |

The two builds differ by ~15 %, which is the compile profile and not the engine — so quote
the **nix** figure for capacity, since that is the binary that ships, and say which build a
number came from. (That is the same discipline as the CPU percentage below, which I got wrong
by two orders of magnitude first: `ms of CPU per second of audio` *is* the core fraction, so
41.3 ms/s is 4.1 % of a core, not 413 %.)

LiveKit publishes 16.5 ms per audio-second for this model. We measure 38.3 — **2.3× the
published figure**, and the honest explanation is the backend: they measure with ONNX
Runtime, we run `tract` because it needs no binary blob. That trade (nix-cleanliness for
2.3× CPU) is worth making at ~26 streams per core and revisiting if the fleet ever gets big
enough to care; `nixpkgs#onnxruntime` 1.27.1 is there if we do.

240 s at 0.031 peak is a **quiet-room baseline, not a false-accept rate**. Four minutes of a
mostly-empty space cannot produce a per-hour figure with any confidence in it, which is why
`bench` refuses to print a rate under 60 s of audio. The number to earn is a busy hour.

## The finding that changed the plan: the phrase alone does not fire

Piper's voice was used as a stand-in speaker (it is already the network's TTS), and the first
results looked like a level problem — the same clip scored 0.063 at full scale and 0.728 when
attenuated. **That reading was wrong**, and the control experiment says so:

| clip (all piper, same model) | peak score |
| --- | --- |
| `hey jarvis` — the phrase alone | **0.063** |
| …with 4 s of real room audio before and after | 0.113 |
| gain swept from 0 dB to −42 dB (eight points) | 0.061 … 0.071 — **flat** |
| `hey jarvis, what is the temperature in the workshop` | **0.626** |
| `hey jarvis, is the space open` | **0.620** |

So gain is irrelevant over 42 dB, and what moves the score is **whether the phrase is inside
an utterance**. The earlier 0.728 was three clips scored in one `bench` run, and `bench` is
deliberately stateful across files — the accumulation looked like an attenuation effect
because both variables moved at once. Fixed-chunk scoring of each clip alone separates them.

The consequence for the product: "hey jarvis" *followed by a question* — the intended
interaction, and what `wake.rs`'s phrase-then-request split assumes — is exactly the shape
that scores 0.62–0.64. Someone saying the two-word phrase alone into a quiet room, three
metres from a camera mic, is the case that may not fire. Whether *real* speech has the same
property is unknown, because piper is a poor stand-in for a person (the same voice that
Whisper transcribes as `K Jarvis`).

## Not measured yet — the gates before anything is enabled

1. **Real human recall**, in the room, at camera distance and level. piper positives are a
   smoke test, not recall.
2. **A busy hour** of room audio through `bench`, and the threshold picked from *our*
   false-accept rate — the way the Frigate `min_area` floor was chosen. LiveKit's own note
   says 0.5–2.5 FA/hour at the default 0.5 threshold on conversational speech; in a space
   with constant talking that is a feature people mute.
3. **cam8's input level against the model's operating band.** The room captured at
   mean **−44.5 dBFS** / max −30.4 dBFS, while LiveKit's positive fixture sits at
   mean −24.5 / max −6.0. If that gap costs recall, the fix is the camera's `rac` audio path
   (its `agc`/`hpf`/`ns` are all `false`) or server-side gain, and it must be decided with
   measurement, not by turning knobs.
4. **Multi-stream scaling** with N devices feeding one process — the ~26/core figure is
   per-stream single-thread; concurrency has its own story.

## Provenance of the models (for the eventual nix pin)

From release `v2.1.3` (published 2026-11-17), `gh release download -R livekit/livekit-wakeword
-p '*.onnx'`:

| file | sha256 | bytes |
| --- | --- | --- |
| `hey_jarvis.onnx` | `8b554c50809888ac14f67fa21acc3bd85e58f5678ee4f2be5d0041f9a2e62843` | 7,406,177 |
| `hey_livekit.onnx` | `6ca579367f62792a5e6d981a10751e6deedd7b2f7a00b08342b7fcd9bb867231` | 7,406,177 |
| `hey_jarvis_micro.tflite` | `d6f3182de754a7afcab030671e01cb04135fe15c540fe9a978d944b8efa65927` | 451,032 |

`hey_jarvis.onnx` is the phrase this fleet wants (`hey jarvis` is already in the wake list,
bench score 0.914). **A second phrase — `computer` — has no published model**: LiveKit's
training CLI needs a CUDA GPU or Apple silicon plus LiveKit Cloud TTS, so a custom phrase is
either a favour, or `sherpa-onnx`'s open-vocabulary keyword spotter (in nixpkgs 1.13.8, ships
`sherpa-onnx-keyword-spotter`, takes a plain keywords file, no training), or `rustpotter`
(trains locally from recordings). Not urgent: it is a nice-to-have.

Only the **streaming** export loads; v1 and openWakeWord classifier files are refused with
`NotStreamingModel`, and the error says so.

## Why this is its own cargo workspace

`crates/Cargo.toml` says it: this workspace has one `Cargo.lock`, and crane reads its
**contents at evaluation** (`cargoLockContents = builtins.readFile`), so a member gaining a
dependency dirties the vendored tree — and therefore the build — of every service on every
box. An ML runtime has no business being in the dependency derivation of `switch-gui`.
`voice-wake` is therefore a workspace of one, with its own lock, and it is deployable to a
single machine.

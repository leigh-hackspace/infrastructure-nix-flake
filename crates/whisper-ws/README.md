# whisper-ws

WebSocket gateway for [whisper.cpp](https://github.com/ggml-org/whisper.cpp) that
speaks the **OpenAI Realtime API** transcription subset, so standard Realtime
clients (or your own) can talk to it without a custom protocol:

```
ws://<host>:8083/v1/realtime?model=whisper-1
```

## Why

whisper.cpp's own server is plain HTTP (`POST /inference`, one shot per
request — no WebSocket, no streaming, verified in 1.8.4 source). `whisper-ws`
is a thin bridge: it accepts OpenAI-style WebSocket events, buffers the
incoming audio, and forwards each committed chunk to whisper-server as a WAV.

- **Zero external crates** (house style): WebSocket (RFC 6455), JSON, base64,
  SHA-1 and the HTTP client are all stdlib Rust in `src/`.
- Runs on aibox in front of `whisper-server`, as `whisper-ws.service` — the unit,
  the ports and the fact that the card it was pinned to was removed (2026-09-02,
  repin pending) are all in `machines/aibox/whisper.nix`.

## Protocol

Client → server:

| Event                       | Purpose                                  |
| --------------------------- | ---------------------------------------- |
| `session.update`            | set `input_audio_transcription.language` |
| `input_audio_buffer.append` | base64 `pcm16` audio @ **24 kHz** mono   |
| `input_audio_buffer.commit` | transcribe the buffered audio            |
| `input_audio_buffer.clear`  | discard the buffer                       |
| `wake_word.probe`           | transcribe an exact audio window (base64 pcm16 @ 24 kHz); not buffered, not recorded |

Server → client: `session.created`, `input_audio_buffer.committed`,
`input_audio_buffer.cleared`, `conversation.item.input_audio_transcription.completed`,
`conversation.item.input_audio_transcription.failed`, `wake_word.probe_result`,
`error` — where `wake_word.probe_result` is the scan of a probe window:
`transcript` + `detected` (or `error`).

Audio is resampled 24 kHz → 16 kHz, then sent to whisper-server as one WAV per
commit (`response_format=verbose_json`, transcript read from `text`). The
buffer auto-commits at `--max-window-secs` (default 120) so a client that
never commits can't grow memory without bound.

## Status page

Plain HTTP on the same port serves the dashboard — a **Solid.js + TypeScript
app** (`web/app`, built with Vite) that polls `/status.json`:

| Endpoint                          | What                                                                                                             |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `GET /`                           | the app (health, live sessions, recent transcriptions; polls /status.json)                                       |
| `GET /assets/*`                   | the app bundle (JS/CSS)                                                                                          |
| `GET /engine/* /ort/* /models/*`  | vendored OpenWakeWord engine, onnxruntime-web, ONNX models (raw modules/WASM — not bundled)                      |
| `GET /status.json`                | JSON snapshot — uptime, whisper-server health, active sessions (incl. in-flight commits), last 20 transcriptions |
| `GET /health`                     | machine-readable health                                                                                          |
| `GET /audio/<session>/<item>.wav` | recorded audio for a recent transcription (16 kHz mono WAV, the same audio whisper-server heard)                 |

The whole dist tree (app + vendored assets) is **embedded in the binary** at
compile time (`web/app/gen_assets.sh` generates `src/assets.rs` with
`include_bytes!`), so the gateway stays a single self-contained binary — no
CDN, nothing leaves the LAN. The flake builds the app automatically
(`buildNpmPackage` + `OWW_DIR`, in `package.nix`); for a plain `cargo build`, run
`web/app/build.sh` first. The wake-word runtime and models are **not** in the
git tree — they are pinned fetches in `package.nix`, so a plain `npm run dev`
needs `OWW_DIR` pointed at them (the one-liner is in
`web/openwakeword/README.md`).

Accessible through the reverse proxy as `https://whisper.int.leighhack.org/`.

The page also has a **WebSocket test** card with a microphone button: press to
record, press again to commit. It streams the mic as 24 kHz PCM16 over the
Realtime WebSocket (same origin, `/v1/realtime`) and shows the transcript — a
quick end-to-end check that the gateway works. While recording, a **live input
level meter** shows the mic level in dBFS so you can confirm the mic is picking
up sound. Recent transcriptions (and the test result) get a **play** button that
replays the recorded audio via `/audio/...`. Mic capture needs the HTTPS URL
(`getUserMedia` requires a secure context).


A **wake word** card listens for "hey jarvis" hands-free: while enabled, the
page runs [OpenWakeWord](https://github.com/dscripka/openWakeWord) (the WASM
engine vendored under `web/openwakeword/`, with the runtime and models fetched
by `package.nix`, served at `/engine`, `/ort` and `/models`) **entirely in the
browser** — the ONNX models
(melspectrogram / embedding / VAD / keyword heads) run through
`onnxruntime-web`, so the server and the GTX 1060 are never involved while
listening. On a match it starts capturing, and a
simple VAD auto-commits the transcript after you pause speaking. Toggle it
on/off from the card. On a match a short confirmation ping plays through the
speakers (the mic is muted for 350 ms so it doesn't land in the recorded
command). Mic capture uses AudioWorkletNode — the page no longer uses the
deprecated ScriptProcessor.

No account or API key is required (unlike the previous Picovoice Porcupine
build). The keyword is set with `WAKE_KEYWORDS` in
`web/app/src/state/controller.ts` — any of: `alexa`, `hey_jarvis`,
`hey_mycroft`, `hey_rhasspy`, `timer`, `weather`. The card shows a clear error
if the models fail to load. Detection needs a Chrome-class browser (WASM SIMD;
the page runs single-threaded, no COOP/COEP headers needed).
`wake_word.probe` still exists server-side for non-browser clients, but the
status page no longer uses it.

## Usage

```
whisper-ws [--bind 127.0.0.1] [--port 8083]
           [--whisper-url http://127.0.0.1:8082] [--max-window-secs 120]
```

## Build

This crate is **not a member of the `crates/` cargo workspace** — `src/assets.rs`
is generated from the built web app, so a member of this workspace could not
compile until npm had run, and cargo reads every member's manifest before it
builds any of them. It opts out with its own `[workspace]` table (the same
mechanism the Dioxus SPAs use) and is packaged by `package.nix` here rather than
by `common/crane.nix`. The long version is in that file's header and in
`docs/building-rust-code.md`.

From the **repository root** (there is no flake in this directory):

```
nix build .#whisper-ws     # web app (npm + vite) then the binary
```

For a plain cargo build (skipping Nix for the web step):

```
nix develop                # from the repo root: cargo + rustc + nodejs
web/app/build.sh           # npm ci + vite build + regenerate src/assets.rs
cargo build                # from this directory (its own workspace, its own lock)
```

The web app is in `web/app` (Solid.js + TypeScript, Vite): `npm run dev` in
there serves it at `http://localhost:5173` for development (the
`/engine /ort /models` assets come from `OWW_DIR`, defaulting to
`web/openwakeword/` — which holds only the engine, so a dev server without the
fetched tree serves the card an engine that cannot load; see that directory's
README); the
`/status.json`, `/v1/realtime` and `/audio/*` endpoints need a running
whisper-ws, so live behaviour is best checked against the deployed box).

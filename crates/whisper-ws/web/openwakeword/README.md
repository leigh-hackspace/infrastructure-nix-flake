# OpenWakeWord (WASM) — the engine, and where the rest comes from

On-device wake word detection for the status page's wake word card. Detection
runs entirely in the browser via `onnxruntime-web`; the server is not involved
while listening. No account or API key required (the previous Picovoice
Porcupine build needed a free `AccessKey` from console.picovoice.ai).

**Only two things are in this directory.** The engine is here because it is
*patched for this repo* — upstream has no copy of it — and this README is here
because it is served to the browser and is the record of what the other 22 MB
was. Everything else is a pinned fetch in `../../package.nix`, which is what the
flake builds into the tree the page loads:

| File | Bytes | Source (pinned in `package.nix`) |
| ---- | ----- | -------------------------------- |
| `engine/WakeWordEngine.js` | 20 KB | [dnavarrom/openwakeword_wasm](https://github.com/dnavarrom/openwakeword_wasm) `src/WakeWordEngine.js`, **patched here** — see below |
| `ort/ort.wasm.min.mjs` | 50 KB | `onnxruntime-web@1.23.2` npm dist (`package/dist/`) |
| `ort/ort-wasm-simd-threaded.mjs` | 20 KB | same |
| `ort/ort-wasm-simd-threaded.wasm` | 11.6 MB | same — the ORT WASM runtime. Runs single-threaded when the page is not cross-origin isolated (no COOP/COEP headers — matches how the page already ran Porcupine) |
| `models/*.onnx` (9 models) | 10.1 MB | [dnavarrom/openwakeword_wasm](https://github.com/dnavarrom/openwakeword_wasm) `models/` at `42b2a2b` (`.onnx` only — the `.tflite` variants are not needed): `melspectrogram.onnx`, `embedding_model.onnx`, `silero_vad.onnx` + keyword heads `alexa_v0.1`, `hey_jarvis_v0.1`, `hey_mycroft_v0.1`, `hey_rhasspy_v0.1`, `timer_v0.1`, `weather_v0.1` |

They were committed here until 2026-10-24. **That is the recovery path**: if a URL
below ever 404s, or the model repo goes away, `git log --stat --
crates/whisper-ws/web/openwakeword` reaches every byte, and the `sha256` of each
one is in `package.nix` (and was checked against the committed copy when the move
was made, so "restore from git" and "fetch from upstream" are the same bytes).
The reason they stopped being committed is the opposite of tidiness: a file under
this directory is inside the flake's own source, so *every evaluation of the
repo* copied all 22 MB into the store — `docs/flake-simplification-pass-2026-10-24.md`
§1 measured 669 MB of those copies on the build machine.

## Files

`engine/WakeWordEngine.js` — the `WakeWordEngine` class (3-stage ONNX pipeline:
melspectrogram → embedding → keyword heads, gated by silero VAD). Patched for
this repo in three places, all of which must be re-applied after an upstream
bump:

1. the ORT import points at `../ort/ort.wasm.min.mjs`;
2. two added public methods — `process(chunk)` (feed one 1280-sample 16 kHz
   Float32Array frame instead of using the AudioWorklet mic path) and `reset()`
   (clear scores/history/cooldown), marked "whisper-ws integration additions";
3. `ort.env.logLevel = 'error'`, to silence ORT's harmless "Unknown CPU vendor"
   warning in wasm builds.

The web app build (`web/app`) copies `engine/`, `ort/`, `models/` and this
README into its dist root, and the whisper-ws binary serves them at `/engine/*`,
`/ort/*` and `/models/*` (see `src/status.rs` `serve_http` + `src/assets.rs`).
The page loads the engine as a runtime ES module (not bundled), so the server
must serve the `.mjs` files as `text/javascript` and the `.wasm` as
`application/wasm`.

## Usage from the page

```js
const { WakeWordEngine } = await import('/engine/WakeWordEngine.js');

const engine = new WakeWordEngine({
  baseAssetUrl: '/models',   // where the .onnx files live
  ortWasmPath: '/ort/',      // trailing slash required
  keywords: ['hey_jarvis'],
  detectionThreshold: 0.5,
  cooldownMs: 2000,
});
await engine.load();
engine.on('detect', ({ keyword, score }) => { /* wake! */ });
// feed 1280-sample (80 ms @ 16 kHz) Float32Array frames:
engine.process(frame);   // fire-and-forget; errors surface via 'error'
// between listening sessions:
engine.reset();
```

The status page keeps its own AudioWorklet mic pipeline (shared with the mic test
card) and feeds the engine a 16 kHz tap (it needs the audio to stream the command
to whisper-ws after the wake word, which the engine's own AudioWorklet path would
swallow).

## Building without Nix

`web/app`'s dev fallback reads these files from `../openwakeword`, and
`ort/`/`models/` are git-ignored, so point the build at the fetched tree instead:

```bash
OWW_DIR=$(nix build --no-link --print-out-paths \
  '.#whisper-ws.passthru.openwakewordAssets') npm run dev
```

…or `npm run build` with the same `OWW_DIR`, which is what
`web/app/build.sh` before a plain `cargo build` wants.

## Browser notes

- Chrome-class browsers with WASM SIMD. The page is not cross-origin isolated,
  so ORT runs single-threaded (`ort-wasm-simd-threaded.*`, the only wasm build
  onnxruntime-web ships). The engine must stream continuously: the keyword
  models need a full ~1.3 s mel/embedding window, so detection lands shortly
  after the wake word is spoken (silence after the utterance helps the score).
- Firefox needs COOP/COEP headers (`Cross-Origin-Opener-Policy` /
  `Cross-Origin-Embedder-Policy: require-corp`) for SharedArrayBuffer, which
  the nginx vhost does not set; wake word detection may not work there.

## License

- `openwakeword_wasm` engine: MIT (see repo).
- OpenWakeWord models: Apache-2.0 (OpenWakeWord project, dscripka).
- `onnxruntime-web`: MIT (Microsoft).

## Updating

Two different things, and only one of them is a fetch:

- **onnxruntime-web**: change `onnxruntimeWebVersion` in `../../package.nix`,
  break the `sha256` on purpose, read the correct one out of the error, and check
  the three filenames in `ortFiles` still exist in `package/dist/` of the new
  version (`npm view onnxruntime-web dist.tarball` names the tarball).
- **Models**: change `openWakeWordRev`, same trick per entry in
  `openWakeWordModels`. The build fails if the count of models changes.
- Computing one by hand instead: **`nix hash file`**, or `sha256sum`, because that
  is the *file's own* digest and what a flat-mode fixed-output derivation
  verifies. `nix hash path` prints a **path** hash — the same file, a different
  number, and `fetchurl` reports it as a hash mismatch that looks exactly like a
  corrupted download. (This cost an hour chasing a CDN: `docs/nix-dev-workflow.md`,
  "Gotchas".)
- **The engine**: copy `src/WakeWordEngine.js` from the repo at the rev above
  over the vendored file and re-apply the three patches listed under "Files" —
  this is the only one of the three that is a manual edit, and the reason the
  file cannot be a fetch.

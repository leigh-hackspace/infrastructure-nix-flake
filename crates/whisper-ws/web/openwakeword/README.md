# OpenWakeWord (WASM) — vendored assets

On-device wake word detection for the status page's wake word card. Detection
runs entirely in the browser via `onnxruntime-web`; the server is not involved
while listening. No account or API key required (the previous Picovoice
Porcupine build needed a free `AccessKey` from console.picovoice.ai).

## Files

| File | Source | Purpose |
| ---- | ------ | ------- |
| `engine/WakeWordEngine.js` | [dnavarrom/openwakeword_wasm](https://github.com/dnavarrom/openwakeword_wasm) `src/WakeWordEngine.js` | `WakeWordEngine` class (3-stage ONNX pipeline: melspectrogram → embedding → keyword heads, gated by silero VAD). Patched for this repo: ORT import points at `../ort/ort.wasm.min.mjs`; two added public methods `process(chunk)` (feed one 1280-sample 16 kHz Float32Array frame instead of using the AudioWorklet mic path) and `reset()` (clear scores/history/cooldown); and `ort.env.logLevel = 'error'` to silence ORT's harmless "Unknown CPU vendor" warning in wasm builds |
| `ort/ort.wasm.min.mjs` | `onnxruntime-web@1.23.2` npm dist (wasm-only ESM build) | ORT JS API (`InferenceSession`, `Tensor`, `env`) |
| `ort/ort-wasm-simd-threaded.mjs` | `onnxruntime-web@1.23.2` npm dist | Emscripten glue, dynamically imported by `ort.wasm.min.mjs` |
| `ort/ort-wasm-simd-threaded.wasm` | `onnxruntime-web@1.23.2` npm dist | The ORT WASM runtime (~12 MB). Runs single-threaded when the page is not cross-origin isolated (no COOP/COEP headers — matches how the page already ran Porcupine) |
| `models/*.onnx` | [dnavarrom/openwakeword_wasm](https://github.com/dnavarrom/openwakeword_wasm) `models/` | OpenWakeWord models: `melspectrogram.onnx`, `embedding_model.onnx`, `silero_vad.onnx` + keyword heads `alexa_v0.1`, `hey_jarvis_v0.1`, `hey_mycroft_v0.1`, `hey_rhasspy_v0.1`, `timer_v0.1`, `weather_v0.1` (`.onnx` only — the `.tflite` variants are not needed) |

The web app build (web/app) copies these files into its dist root and the
whisper-ws binary serves them at `/engine/*`, `/ort/*` and `/models/*` (see
`src/status.rs` `serve_http` + `src/assets.rs`). The page loads the engine as
a runtime ES module (not bundled), so the server must serve the `.mjs` files
as `text/javascript` and the `.wasm` as `application/wasm`.

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

The status page keeps its own AudioWorklet mic pipeline (shared with the
mic test card) and feeds the engine a 16 kHz tap (it needs the audio to
stream the command to whisper-ws after the wake word, which the engine's own
AudioWorklet path would swallow).

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

```bash
# Engine + models
git clone https://github.com/dnavarrom/openwakeword_wasm /tmp/openwakeword_wasm
cp /tmp/openwakeword_wasm/src/WakeWordEngine.js web/openwakeword/engine/
cp /tmp/openwakeword_wasm/models/*.onnx web/openwakeword/models/
# re-apply the two patches in engine/WakeWordEngine.js:
#   1) import * as ort from '../ort/ort.wasm.min.mjs';
#   2) the public process()/reset() methods (marked "whisper-ws integration additions")

# onnxruntime-web
npm pack onnxruntime-web@<version>   # or whatever the latest is
tar -xzf onnxruntime-web-*.tgz
cp package/dist/ort.wasm.min.mjs web/openwakeword/ort/
cp package/dist/ort-wasm-simd-threaded.mjs web/openwakeword/ort/
cp package/dist/ort-wasm-simd-threaded.wasm web/openwakeword/ort/
git add web/openwakeword/
```

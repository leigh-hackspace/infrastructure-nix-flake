// The page's live state: one shared microphone pipeline, one shared Realtime
// WebSocket, and two modes (mic "test" and "wake word") that can't run at the
// same time. Ported 1:1 from the old single-script status page to Solid
// signals; the cards are thin views over these.

import { createSignal } from 'solid-js';
import { MicCapture } from '../audio/mic';
import { makeResampler, resetResampler, resampleChunkTo, type ResampleState } from '../audio/resample';
import { b64 } from '../lib/format';
import { realtime } from './realtime';
import type { RealtimeMessage } from '../types';

export type Mode = 'idle' | 'test' | 'wake';
export type TextKind = '' | 'txt-ok' | 'txt-wait' | 'txt-err';
export interface TextMsg {
  text: string;
  kind: TextKind;
}
export interface ResultMsg {
  text: string;
  kind: TextKind;
  /** Play button for the recorded clip of the latest result, if any. */
  playUrl?: string;
}

// --- Constants (same values as the old page) ---
const TARGET_RATE = 24000; // gateway input rate
const WAKE_KEYWORDS = ['hey_jarvis']; // any of: alexa, hey_jarvis, hey_mycroft, hey_rhasspy, timer, weather
export const WAKE_WORD_LABEL = WAKE_KEYWORDS.join(' / ');
const OWW_FRAME_SIZE = 1280; // engine frame = 80 ms @ 16 kHz
const OWW_DETECTION_THRESHOLD = 0.5; // keyword score required to fire
const OWW_COOLDOWN_MS = 2000; // ignore re-detections for this long
const SPEECH_DB = -40; // RMS dB above which a chunk counts as speech
const END_SILENCE_MS = 1600; // pause after speech that ends the command
const MIN_CLIP_S = 0.6; // ignore sub-second noise after the wake word
const MIN_SPEECH_S = 0.5; // speech (not wake-word tail) required to commit
const MAX_CLIP_S = 20; // safety cap on command length
const PING_MUTE_MS = 350; // mic mute after the confirmation ping

// --- Shared signals (read by both cards) ---
export const [mode, setMode] = createSignal<Mode>('idle');
export const [recording, setRecording] = createSignal(false);
export const [bufferedSecs, setBufferedSecs] = createSignal(0);

// Input level meter: live RMS of the mic chunk in dBFS; the white tick holds
// the recent peak so brief input is still visible.
export const [levelDb, setLevelDb] = createSignal(-100);
export const [levelPeakDb, setLevelPeakDb] = createSignal(-60);
export const [nanSamples, setNanSamples] = createSignal(0);
let peakDb = -60;
let peakAt = 0;

// --- "WebSocket test" card state ---
export const [testBusy, setTestBusy] = createSignal(false);
export const [testStatus, setTestStatus] = createSignal<TextMsg>({
  text: 'idle — press the mic, speak, press again to transcribe',
  kind: '',
});
export const [testResult, setTestResult] = createSignal<ResultMsg>({ text: '', kind: '' });

// --- "Wake word" card state ---
export const [wakeBusy, setWakeBusy] = createSignal(false);
export const [wakeStatus, setWakeStatus] = createSignal<TextMsg>({ text: 'off — click to listen', kind: '' });
export const [wakeResult, setWakeResult] = createSignal<ResultMsg>({ text: '', kind: '' });

// --- Internals (mirrors the old page's module-level vars) ---
const mic = new MicCapture();
let resampler: ResampleState = makeResampler(); // 24 kHz gateway stream
let resampler16: ResampleState | null = null; // 16 kHz wake-word tap
let wakeEngine: WakeWordEngineLike | null = null; // created + loaded once
let wakeEngineReady = false; // models loaded and sessions warm
let wakeBuf: number[] = []; // pending 16 kHz samples for engine frames
let wakeTriggered = false; // wake word heard; capturing the command
let speechSeen = false; // speech detected since the wake word
let speechSecs = 0; // accumulated speech since the wake word
let lastSpeechAt = 0; // performance.now() of the last speech chunk
let pingMuteUntil = 0;

// Minimal structural type for the vendored engine (web/openwakeword/engine/
// WakeWordEngine.js) so the dynamic import needs no ambient declaration.
interface WakeWordEngineLike {
  load(): Promise<void>;
  process(frame: Float32Array): void;
  reset(): void;
  on(event: 'detect', cb: (ev: { keyword: string; score: number }) => void): void;
  on(event: 'error', cb: (err: unknown) => void): void;
  on(event: 'ready', cb: () => void): void;
}

function failSafe(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// --- Input level: RMS of the raw mic chunk mapped to dBFS. Bar is green
// below -30 dB, amber through -12 dB, red approaching clipping. NaN samples
// from the browser (a known Chromium capture quirk) count as silence. ---
function updateLevel(rms: number): void {
  if (!Number.isFinite(rms)) rms = 0;
  let db = rms > 0 ? 20 * Math.log10(rms + 1e-9) : -100;
  db = Math.max(db, -100);
  const now = performance.now();
  if (db > peakDb || now - peakAt > 1500) {
    peakDb = db;
    peakAt = now;
  }
  setLevelDb(db);
  setLevelPeakDb(peakDb);
}

function rmsOf(f32: Float32Array): number {
  let sum = 0;
  let n = 0;
  for (const v of f32) {
    if (v !== v || v === Infinity || v === -Infinity) continue; // NaN/±Inf
    sum += v * v;
    n++;
  }
  return n ? Math.sqrt(sum / n) : 0;
}

function countNaN(f32: Float32Array): number {
  let n = 0;
  for (const v of f32) if (v !== v) n++;
  return n;
}

function resetLevel(): void {
  peakDb = -60;
  peakAt = 0;
  setLevelDb(-100);
  setLevelPeakDb(-60);
  setNanSamples(0);
}

function resetCommon(): void {
  resetResampler(resampler);
  setBufferedSecs(0);
  resetLevel();
}

function micReset(): void {
  setRecording(false);
  void mic.stop();
}

// --- Microphone test: streams the mic as 24 kHz PCM16 over the Realtime
// WebSocket and shows the transcript, proving the gateway end-to-end. ---
export async function startTest(): Promise<void> {
  if (mode() === 'wake') stopWake();
  setTestResult({ text: '', kind: '' });
  setTestBusy(true);
  setTestStatus({ text: `connecting · ${location.host}…`, kind: 'txt-wait' });
  try {
    await realtime.connect();
    setMode('test');
    await mic.start(handleAudioChunk);
    if (!realtime.isOpen) throw new Error('websocket closed');
    resetCommon();
    setRecording(true);
    setTestStatus({ text: 'connected · recording — speak now … 0.0s buffered', kind: 'txt-ok' });
  } catch (e) {
    failTest(failSafe(e));
  } finally {
    setTestBusy(false);
  }
}

export function stopTest(): void {
  micReset();
  setMode('idle');
  if (realtime.isOpen) {
    realtime.send({ type: 'input_audio_buffer.commit' });
    setTestStatus(
      bufferedSecs() < 0.3
        ? { text: 'committed · clip too short — say a bit more next time', kind: 'txt-wait' }
        : { text: 'committed · transcribing…', kind: 'txt-wait' },
    );
  } else {
    setTestStatus({ text: 'not connected', kind: 'txt-err' });
  }
}

function failTest(msg: string): void {
  micReset();
  setMode('idle');
  setTestStatus({ text: 'failed', kind: 'txt-err' });
  setTestResult({ text: `❌ ${msg}`, kind: 'txt-err' });
}

// --- Wake word mode: listens for the configured keyword(s), then transcribes
// what follows. Detection runs entirely client-side with OpenWakeWord (WASM
// engine + onnxruntime-web, vendored under web/openwakeword; see
// web/openwakeword/README.md) — no access key, nothing leaves the LAN. The
// engine spots the keyword in a 16 kHz tap of the mic stream, so while
// listening the server is never involved — the GTX 1060 stays free for the
// command transcription itself. On a match the page captures the command, and
// a simple VAD auto-commits after a pause. ---

// First use creates the engine and downloads the ONNX models
// (melspectrogram / embedding / VAD + keyword heads) from /models. Loaded once
// per page; later wake sessions reuse the warm sessions
// (wakeEngine.reset() clears scores/cooldown between runs).
async function initWakeEngine(): Promise<void> {
  if (wakeEngine) {
    await wakeEngine.load();
    wakeEngineReady = true;
    return;
  }
  // Runtime ES-module import: the engine and onnxruntime-web are vendored
  // assets served by whisper-ws (vite.config.ts publicDir), deliberately not
  // bundled — the .mjs/.wasm must be fetched by their own paths at runtime.
  const engineUrl = '/engine/WakeWordEngine.js';
  const mod = (await import(/* @vite-ignore */ engineUrl)) as {
    WakeWordEngine: new (cfg: Record<string, unknown>) => WakeWordEngineLike;
  };
  wakeEngine = new mod.WakeWordEngine({
    baseAssetUrl: '/models',
    ortWasmPath: '/ort/',
    keywords: WAKE_KEYWORDS,
    detectionThreshold: OWW_DETECTION_THRESHOLD,
    cooldownMs: OWW_COOLDOWN_MS,
  });
  wakeEngine.on('detect', onKeywordDetected);
  wakeEngine.on('error', (err) => {
    console.error('openwakeword:', err);
    if (mode() === 'wake') {
      setWakeResult({
        text: `⚠ engine error: ${err instanceof Error ? err.message : String(err)}`,
        kind: 'txt-err',
      });
    }
  });
  wakeEngine.on('ready', () => {
    wakeEngineReady = true;
  });
  await wakeEngine.load();
  wakeEngineReady = true;
}

export async function startWake(): Promise<void> {
  if (mode() === 'wake') return;
  if (recording()) stopTest();
  setMode('wake');
  setWakeBusy(true);
  setWakeResult({ text: '', kind: '' });
  setWakeStatus({ text: `starting · ${location.host}…`, kind: 'txt-wait' });
  try {
    await realtime.connect();
    await mic.start(handleAudioChunk);
    if (!realtime.isOpen) throw new Error('websocket closed');
    await initWakeEngine();
    // Toggled off while the engine was initialising — bail out silently.
    if (mode() !== 'wake') {
      micReset();
      return;
    }
    wakeEngine!.reset(); // clear scores / history / cooldown
    resampler16 = makeResampler();
    wakeBuf = [];
    wakeTriggered = false;
    speechSeen = false;
    speechSecs = 0;
    lastSpeechAt = 0;
    resetCommon();
    setRecording(true);
    setWakeStatus({ text: `listening for “${WAKE_WORD_LABEL}” …`, kind: 'txt-ok' });
  } catch (e) {
    failWake(failSafe(e));
  } finally {
    setWakeBusy(false);
  }
}

export function stopWake(): void {
  if (mode() !== 'wake') return;
  if (wakeEngine) wakeEngine.reset();
  wakeBuf = [];
  wakeTriggered = false;
  speechSeen = false;
  speechSecs = 0;
  micReset();
  setWakeStatus({ text: `off — click to listen for “${WAKE_WORD_LABEL}”`, kind: '' });
  setMode('idle');
}

function failWake(msg: string): void {
  stopWake();
  setWakeStatus({ text: 'failed', kind: 'txt-err' });
  setWakeResult({ text: `❌ ${msg}`, kind: 'txt-err' });
}

// Feed the engine exact 1280-sample (80 ms @ 16 kHz) frames from the 16 kHz
// tap. process() is fire-and-forget: the engine serialises frames through an
// internal promise queue and runs far faster than real time, so the queue
// never builds up. Stops once the wake word has fired (state is reset on
// re-arm via startWake()/wakeResume()).
function feedWakeEngine(samples: number[]): void {
  if (!wakeEngine || !wakeEngineReady || wakeTriggered) return;
  wakeBuf.push(...samples);
  while (wakeBuf.length >= OWW_FRAME_SIZE) {
    const frame = new Float32Array(OWW_FRAME_SIZE);
    for (let j = 0; j < OWW_FRAME_SIZE; j++) frame[j] = wakeBuf[j];
    wakeBuf = wakeBuf.slice(OWW_FRAME_SIZE);
    wakeEngine.process(frame);
  }
}

function onKeywordDetected(ev: { keyword: string; score: number }): void {
  // Ignore detections while already capturing (the command may contain the
  // keyword) or once the card has been turned off.
  if (mode() !== 'wake' || wakeTriggered) return;
  console.log(`wake word: ${ev.keyword} (score ${ev.score.toFixed(2)})`);
  onWakeDetected();
}

function onWakeDetected(): void {
  wakeTriggered = true;
  speechSeen = false;
  speechSecs = 0;
  lastSpeechAt = 0;
  // Fresh start for the command: while listening, wake mode streams nothing
  // to the session buffer, so there is nothing accumulated to drop.
  realtime.send({ type: 'input_audio_buffer.clear' });
  setBufferedSecs(0);
  wakeBuf = [];
  setWakeResult({ text: '', kind: '' });
  playPing();
  pingMuteUntil = performance.now() + PING_MUTE_MS;
  setWakeStatus({ text: `👌 “${WAKE_WORD_LABEL}” detected — speak, then pause …`, kind: 'txt-wait' });
}

function wakeCommit(): void {
  if (mode() !== 'wake' || !wakeTriggered) return;
  wakeTriggered = false; // don't re-trigger on the tail of this clip
  if (realtime.isOpen) {
    realtime.send({ type: 'input_audio_buffer.commit' });
    setWakeStatus({ text: 'committed · transcribing…', kind: 'txt-wait' });
  } else {
    wakeResume();
  }
}

function wakeResume(): void {
  if (mode() !== 'wake') return;
  wakeTriggered = false;
  speechSeen = false;
  speechSecs = 0;
  lastSpeechAt = 0;
  wakeBuf = [];
  setBufferedSecs(0);
  if (wakeEngine) wakeEngine.reset();
  setWakeStatus({ text: `listening for “${WAKE_WORD_LABEL}” …`, kind: 'txt-ok' });
}

// Wake-word confirmation ping (Web Audio, no asset needed). The mic is muted
// for PING_MUTE_MS after a detection so the ping doesn't land in the recorded
// command.
function playPing(): void {
  const ctx = mic.context;
  if (!ctx || ctx.state !== 'running') return;
  const t = ctx.currentTime;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = 'sine';
  osc.frequency.setValueAtTime(880, t);
  osc.frequency.exponentialRampToValueAtTime(1320, t + 0.12);
  gain.gain.setValueAtTime(0.0001, t);
  gain.gain.exponentialRampToValueAtTime(0.4, t + 0.02);
  gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.3);
  osc.connect(gain);
  gain.connect(ctx.destination);
  osc.start(t);
  osc.stop(t + 0.32);
  osc.onended = () => {
    try {
      osc.disconnect();
    } catch {
      /* already gone */
    }
    try {
      gain.disconnect();
    } catch {
      /* already gone */
    }
  };
}

// One mic chunk (~4096 samples at the AudioContext rate). Computes the level
// meter, resamples to 24 kHz for the gateway, and — in wake mode — taps 16
// kHz for the engine plus runs the commit VAD.
function handleAudioChunk(input: Float32Array): void {
  if (!recording()) return;
  setNanSamples((n) => n + countNaN(input));
  const rms = rmsOf(input);
  updateLevel(rms);
  const out = resampleChunkTo(input, resampler, mic.sampleRate, TARGET_RATE);
  if (!out.length) return;
  const bytes = new Uint8Array(out.length * 2);
  const dv = new DataView(bytes.buffer);
  for (let i = 0; i < out.length; i++) dv.setInt16(i * 2, out[i], true);

  if (mode() === 'wake') {
    // While listening, only the wake-word engine sees the mic (16 kHz tap);
    // the session buffer stays empty until the wake word fires, so no
    // periodic clears are needed. The GTX 1060 is not touched until the
    // command is committed.
    feedWakeEngine(resampleChunkTo(input, resampler16!, mic.sampleRate, 16000));
    if (wakeTriggered) {
      // The confirmation ping plays right after detection; drop mic chunks
      // during the mute window so it can't land in the recorded command.
      if (performance.now() < pingMuteUntil) return;
      if (realtime.isOpen) {
        realtime.send({ type: 'input_audio_buffer.append', audio: b64(bytes) });
        setBufferedSecs((b) => b + out.length / TARGET_RATE);
      }
      // Simple VAD: end the command after a pause (or the safety cap). The
      // MIN_SPEECH_S gate keeps the wake word's own tail from being committed
      // as a command when the user pauses before actually speaking.
      const db = rms > 0 ? 20 * Math.log10(rms + 1e-9) : -100;
      const now = performance.now();
      if (db > SPEECH_DB) {
        speechSeen = true;
        lastSpeechAt = now;
        speechSecs += out.length / TARGET_RATE;
      }
      if (
        speechSeen &&
        speechSecs >= MIN_SPEECH_S &&
        bufferedSecs() >= MIN_CLIP_S &&
        (now - lastSpeechAt > END_SILENCE_MS || bufferedSecs() >= MAX_CLIP_S)
      ) {
        wakeCommit();
      }
      setWakeStatus({
        text: `👌 “${WAKE_WORD_LABEL}” detected — speak, then pause … ${bufferedSecs().toFixed(1)}s`,
        kind: 'txt-wait',
      });
    }
  } else {
    if (realtime.isOpen) {
      realtime.send({ type: 'input_audio_buffer.append', audio: b64(bytes) });
      setBufferedSecs((b) => b + out.length / TARGET_RATE);
    }
    setTestStatus({
      text: `connected · recording — speak now … ${bufferedSecs().toFixed(1)}s buffered`,
      kind: 'txt-ok',
    });
  }
}

// --- WebSocket events (one shared handler, routed by mode — as before) ---
// item_id of the most recent transcription; its recorded clip plays back
// from /audio/<session>/<item>.wav (the server keeps it in the recent window).
let pendingItemId: string | undefined;

function resultWithPlay(text: string, kind: TextKind): ResultMsg {
  const playUrl =
    realtime.sessionId && pendingItemId !== undefined ? `/audio/${realtime.sessionId}/${pendingItemId}.wav` : undefined;
  return { text, kind, playUrl };
}

function onWsMessage(msg: RealtimeMessage): void {
  switch (msg.type) {
    case 'session.created':
      if (mode() === 'wake') {
        setWakeStatus({ text: `listening for “${WAKE_WORD_LABEL}” …`, kind: 'txt-ok' });
      } else {
        setTestStatus({ text: 'connected', kind: 'txt-ok' });
      }
      break;
    case 'input_audio_buffer.committed':
      if (!recording()) {
        if (mode() === 'wake') setWakeStatus({ text: 'committed · transcribing…', kind: 'txt-wait' });
        else setTestStatus({ text: 'committed · transcribing…', kind: 'txt-wait' });
      }
      break;
    case 'conversation.item.input_audio_transcription.completed':
      pendingItemId = msg.item_id;
      if (mode() === 'wake') {
        setWakeStatus({ text: 'transcribed', kind: 'txt-ok' });
        setWakeResult(resultWithPlay(`💬 ${msg.transcript}`, 'txt-ok'));
        wakeResume();
      } else {
        // Not wake mode → the test card owns the result (even after the
        // commit stopped the recording).
        setTestStatus({ text: 'transcribed', kind: 'txt-ok' });
        setTestResult(resultWithPlay(`💬 ${msg.transcript}`, 'txt-ok'));
      }
      break;
    case 'conversation.item.input_audio_transcription.failed':
      if (mode() === 'wake') {
        setWakeStatus({ text: 'transcription failed', kind: 'txt-err' });
        setWakeResult({ text: `❌ ${msg.error?.message || 'failed'}`, kind: 'txt-err' });
        wakeResume();
      } else {
        setTestStatus({ text: 'transcription failed', kind: 'txt-err' });
        setTestResult({ text: `❌ ${msg.error?.message || 'failed'}`, kind: 'txt-err' });
      }
      break;
    case 'error':
      if (mode() === 'wake') {
        setWakeResult({ text: `❌ ${msg.error?.message || 'server error'}`, kind: 'txt-err' });
      } else {
        setTestResult({ text: `❌ ${msg.error?.message || 'server error'}`, kind: 'txt-err' });
      }
      break;
  }
}

function onWsClose(ev: CloseEvent): void {
  const wasWake = mode() === 'wake';
  const wasRecording = recording();
  if (wasWake) {
    stopWake();
    setWakeStatus({ text: `websocket closed (code ${ev.code})`, kind: 'txt-err' });
    setWakeResult({ text: '❌ connection lost mid-session', kind: 'txt-err' });
  } else {
    micReset();
    if (wasRecording) {
      setTestStatus({ text: `websocket closed (code ${ev.code})`, kind: 'txt-err' });
      setTestResult({ text: '❌ connection lost mid-test', kind: 'txt-err' });
    } else {
      setTestStatus({ text: 'disconnected', kind: '' });
    }
  }
}

realtime.onMessage(onWsMessage);
realtime.onClose(onWsClose);

// --- Button handlers (same toggle semantics as the old page) ---
export function onMicClick(): void {
  if (mode() === 'wake') {
    stopWake();
    void startTest();
    return;
  }
  if (recording()) stopTest();
  else void startTest();
}

export function onWakeClick(): void {
  if (mode() === 'wake') stopWake();
  else void startWake();
}

export function levelPct(db: number): number {
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

export function levelColor(db: number): string {
  return db < -30 ? 'var(--good)' : db < -12 ? 'var(--wait)' : 'var(--bad)';
}

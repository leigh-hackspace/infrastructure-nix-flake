import { createEffect, createSignal, untrack } from 'solid-js';
import type { Accessor } from 'solid-js';
import { MicCapture } from '../audio/mic';
import { makeResampler, resetResampler, resampleChunkTo } from '../audio/resample';
import { wakeResult } from './controller';

/**
 * The voice assistant, in the browser: press and talk, or let the wake-word card
 * hand us a transcript, and the gateway answers out loud.
 *
 * Everything *hard* is on the server (`crates/voice-assistant/`): whisper.cpp for the
 * transcription, the bot's tools for the answer, piper for the voice, and the rule
 * about how long an answer may be. This file's whole job is the microphone, the WAV,
 * and the fetches — which is deliberate: the browser is the one place with a real
 * echo canceller and a real speaker, so it should hold as little policy as possible.
 *
 * Two ways in, on purpose:
 *
 * - **Push to talk** proves the pipeline and needs nothing but a click, which is what
 *   you want when the audio path is new and someone has to tell "broken" from "you
 *   spoke too quietly".
 * - **Hands-free** subscribes to the wake-word card's result. The card already runs
 *   OpenWakeWord and already streams the command to whisper-ws for a transcript, so
 *   arming this costs no second recorder and no second detector — the wake word and
 *   the transcription are simply events this listens for.
 */

/** What the gateway's `/v1/transcribe` wants and what the camera speaks. */
const TARGET_RATE = 16000;

/** Silence, in ms, after which push-to-talk stops by itself. */
const STOP_AFTER_QUIET_MS = 900;
/** Longest single push-to-talk recording. */
const MAX_RECORD_MS = 15000;
/** dBFS below which a frame counts as silence for the auto-stop. */
const QUIET_DB = -42;

let gatewayUrl = '';

/** Called from the card with `status.json`'s `assistant_url` on every poll. */
export function configureGateway(url: string): void {
  const next = (url || '').replace(/\/+$/, '');
  if (next === gatewayUrl) return;
  gatewayUrl = next;
  setGatewayKnown(!!next);
  if (!next) setStatus('the gateway URL is not configured on the server');
}

export const [gatewayKnown, setGatewayKnown] = createSignal(false);
export const [recording, setRecording] = createSignal(false);
export const [busy, setBusy] = createSignal('');
export const [status, setStatus] = createSignal('idle');
export const [question, setQuestion] = createSignal('');
export const [answer, setAnswer] = createSignal('');
export const [timing, setTiming] = createSignal('');
export const [handsFree, setHandsFree] = createSignal(false);
export const [levelDb, setLevelDb] = createSignal(-100);

function humanError(e: unknown): string {
  const msg = e instanceof Error ? e.message : String(e);
  return msg.startsWith('TypeError') ? `cannot reach ${gatewayUrl || 'the gateway'} — is voice.int.leighhack.org up?` : msg;
}

async function post(path: string, body: BodyInit, contentType: string): Promise<Response> {
  if (!gatewayUrl) throw new Error('the gateway URL is not configured on the server');
  const r = await fetch(gatewayUrl + path, { method: 'POST', body, headers: { 'content-type': contentType } });
  if (!r.ok) {
    let detail = '';
    try {
      detail = (await r.json()).error ?? '';
    } catch {
      /* an HTML error page from the front door is still worth the status */
    }
    throw new Error(`${r.status}${detail ? `: ${detail}` : ''}`);
  }
  return r;
}

/** A PCM16 mono WAV from the collected samples, at 16 kHz. */
function wav(samples: number[]): Uint8Array {
  const n = samples.length;
  const buf = new ArrayBuffer(44 + n * 2);
  const view = new DataView(buf);
  const ascii = (at: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(at + i, s.charCodeAt(i));
  };
  ascii(0, 'RIFF');
  view.setUint32(4, 36 + n * 2, true);
  ascii(8, 'WAVE');
  ascii(12, 'fmt ');
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, TARGET_RATE, true);
  view.setUint32(28, TARGET_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  ascii(36, 'data');
  view.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) view.setInt16(44 + i * 2, samples[i], true);
  return new Uint8Array(buf);
}

async function speak(b64: string | null): Promise<void> {
  if (!b64) return;
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const blob = new Blob([bytes], { type: 'audio/wav' });
  const url = URL.createObjectURL(blob);
  const audio = new Audio(url);
  // Revoked when it is done rather than in a cleanup: this runs from a click handler,
  // outside any component owner, so there is nothing for onCleanup to attach to.
  audio.addEventListener('ended', () => URL.revokeObjectURL(url));
  await audio.play();
}

/** Ask a question that is already text — the hands-free path. */
export async function askText(text: string, source: string): Promise<void> {
  if (busy()) return;
  const q = text.trim();
  if (!q) return;
  setQuestion(q);
  setAnswer('');
  setBusy('asking…');
  setStatus(source);
  const started = performance.now();
  try {
    const r = await post('/v1/ask', JSON.stringify({ text: q, voice: true }), 'application/json');
    const d = await r.json();
    const total = ((performance.now() - started) / 1000).toFixed(1);
    setAnswer(d.answer);
    setTiming(
      `${total}s here · ${d.agent_secs}s model · ${d.engine} · ${d.rounds} round(s)` +
        (d.tools?.length ? ` · ${d.tools.join(', ')}` : ''),
    );
    if (d.warnings?.length) setStatus(`warning: ${d.warnings.join('; ')}`);
    setBusy('');
    await speak(d.audio ?? null);
  } catch (e) {
    setBusy('');
    setStatus(`ask failed: ${humanError(e)}`);
  }
}

// --- push to talk -------------------------------------------------------------

const mic = new MicCapture();
let samples: number[] = [];
let resampler = makeResampler();
let quietMs = 0;
let timer = 0;

function frameDb(f32: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < f32.length; i++) sum += f32[i] * f32[i];
  const rms = Math.sqrt(sum / Math.max(1, f32.length));
  return rms <= 1e-6 ? -120 : 20 * Math.log10(rms);
}

export async function startTalk(): Promise<void> {
  if (recording() || busy()) return;
  if (!gatewayKnown()) {
    setStatus('the gateway URL is not configured — set whisper-ws --assistant-url');
    return;
  }
  samples = [];
  resetResampler(resampler);
  quietMs = 0;
  setBusy('');
  try {
    await mic.start((chunk) => {
      const db = frameDb(chunk);
      setLevelDb(db);
      const out = resampleChunkTo(chunk, resampler, mic.sampleRate, TARGET_RATE);
      for (let i = 0; i < out.length; i++) samples.push(out[i]);
      quietMs = db < QUIET_DB ? quietMs + (chunk.length / mic.sampleRate) * 1000 : 0;
      const secs = samples.length / TARGET_RATE;
      if (quietMs > STOP_AFTER_QUIET_MS || secs * 1000 > MAX_RECORD_MS) void stopTalk();
      else setStatus(`listening — ${secs.toFixed(1)}s`);
    });
    setRecording(true);
    setStatus('listening — talk now, or press again to stop');
  } catch (e) {
    setStatus(`microphone failed: ${humanError(e)}`);
    setRecording(false);
  }
}

export async function stopTalk(): Promise<void> {
  if (!recording()) return;
  setRecording(false);
  await mic.stop();
  window.clearTimeout(timer);
  const n = samples.length;
  const secs = n / TARGET_RATE;
  setStatus('transcribing…');
  setBusy('transcribing…');
  const started = performance.now();
  try {
    if (secs < 0.3) throw new Error(`only ${secs.toFixed(1)}s of audio — say it a little longer`);
    const r = await post('/v1/transcribe', wav(samples), 'audio/wav');
    const d = await r.json();
    const stt = ((performance.now() - started) / 1000).toFixed(1);
    if (!d.speech) {
      setBusy('');
      setStatus(`not speech (${d.text || 'nothing'}) — whisper heard ${d.text || 'noise'}`);
      return;
    }
    setTiming(`transcribed in ${stt}s`);
    await askText(d.text, 'push to talk');
  } catch (e) {
    setBusy('');
    setStatus(`transcribe failed: ${humanError(e)}`);
  } finally {
    samples = [];
  }
}

export function onTalkClick(): void {
  void (recording() ? stopTalk() : startTalk());
}

// --- hands-free ---------------------------------------------------------------

let lastSeen = '';

/**
 * Watch the wake-word card. Called once from the card's setup, so the effect belongs
 * to a component owner and is disposed with the page.
 *
 * It is an effect rather than a poll because `wakeResult` is a signal the wake-word
 * card already writes: the transcription arrives once, and an effect runs once per
 * change, which is what "did the room just wake us up" needs.  The equality check is
 * still there because the wake card's result persists — without it, arming the
 * assistant would immediately re-answer the last thing said before anyone armed it.
 */
export function watchWakeResult(): void {
  createEffect(() => {
    const t = wakeResult().text;
    if (!handsFree() || !t || t === lastSeen || busy() || recording()) return;
    lastSeen = t;
    void askText(t, 'wake word');
  });
}

export function toggleHandsFree(): void {
  const next = !handsFree();
  setHandsFree(next);
  setStatus(next ? 'armed — say the wake word' : 'disarmed');
  // Arming must not answer what was said before arming.
  lastSeen = untrack(wakeResult)().text;
}

export const level: Accessor<number> = levelDb;

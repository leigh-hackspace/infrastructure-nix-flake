import { Show, createEffect } from 'solid-js';
import type { Accessor } from 'solid-js';
import type { StatusSnapshot } from '../types';
import { Card, Pill } from './Card';
import { LevelMeter } from './LevelMeter';
import { WAKE_WORD_LABEL } from '../state/controller';
import {
  answer,
  askText,
  busy,
  chooseWake,
  configureGateway,
  gatewayKnown,
  level,
  onTalkClick,
  question,
  recording,
  serverDetections,
  serverScore,
  status,
  timing,
  wakeEngine,
  watchWakeResult,
} from '../state/assistant';

/**
 * The assistant: ask out loud, hear the answer.
 *
 * Push to talk is the honest first control — one click, one recording, one answer —
 * and hands-free is an arm switch over the wake-word card above it, not a second
 * detector (see `state/assistant.ts` for why that matters: that card already has the
 * keyword running and already gets a transcript).
 *
 * The timing line is the point of this card for whoever maintains it. A voice
 * assistant lives or dies on how long the room waits, and the split — transcription,
 * model, total — is the only way to see which of the three is slow today.
 */
export function AssistantCard(props: { snapshot: Accessor<StatusSnapshot | null> }) {
  // The gateway's origin is a whisper-ws flag, not something written into the bundle:
  // a hostname in the SPA is a fact that can only be fixed by rebuilding it.
  createEffect(() => configureGateway(props.snapshot()?.assistant_url ?? ''));
  watchWakeResult();

  return (
    <Card title="Voice assistant">
      <Show
        when={gatewayKnown()}
        fallback={
          <p class="sub txt-err">
            No gateway configured — run whisper-ws with <code>--assistant-url</code> pointing at{' '}
            <code>voice.int.leighhack.org</code>.
          </p>
        }
      >
        <p class="sub">
          Click <b>Talk</b>, speak, then click <b>Stop and ask</b> — or just pause for a second and it
          sends itself. Or pick a wake source: <b>this tab</b> runs OpenWakeWord here, <b>server</b>{' '}
          streams your microphone to the network's own detector, which is the same engine a
          camera with no CPU gets.
        </p>
        <div class="row">
          <button class={`mic ${recording() ? 'on' : ''}`} disabled={!!busy()} onClick={onTalkClick}>
            {recording() ? '🔴 Stop and ask' : '🎙 Talk'}
          </button>
          {/* One control, three states, because two listeners on one microphone is two
              answers to one sentence. `this tab` is OpenWakeWord running here; `server` hands
              the stream to voice-wake on services1 — the path a camera or any dumb device
              takes, which is why it is reachable from any browser before it is wired to one. */}
          <select
            style={{ 'font-size': '.95rem', 'font-family': 'inherit' }}
            disabled={!!busy()}
            value={wakeEngine()}
            onChange={(e) => void chooseWake(e.currentTarget.value as 'off' | 'local' | 'server')}
          >
            <option value="off">👂 wake: off</option>
            <option value="local">👂 wake: this tab</option>
            <option value="server">👂 wake: server</option>
          </select>
        </div>
        <Show when={wakeEngine() === 'server'}>
          <div class="sub">
            server score{' '}
            <Pill kind={serverScore() >= 0.5 ? 'good' : 'wait'}>{serverScore().toFixed(2)}</Pill>{' '}
            · detections {serverDetections()} — this tab is streaming its microphone to{' '}
            <code>voice.int.leighhack.org</code>
          </div>
        </Show>
        <LevelMeter visible={() => recording()} />
        <div class="sub">{status()}</div>
        <Show when={question()}>
          <div class="result txt-ok">“{question()}”</div>
        </Show>
        <Show when={busy()}>
          <div class="sub txt-wait">{busy()}</div>
        </Show>
        <Show when={answer()}>
          <div class="result txt-ok">{answer()}</div>
          <Show when={timing()}>
            <div class="sub txt-wait">{timing()}</div>
          </Show>
        </Show>
        <Show when={recording()}>
          <div class="sub">
            <Pill kind={level() > -30 ? 'good' : 'wait'}>{level().toFixed(0)} dBFS</Pill>
          </div>
        </Show>
        <details>
          <summary>Or type it</summary>
          <TextAsk />
        </details>
      </Show>
    </Card>
  );
}

/** The same question by keyboard, so a room full of noise is not a dead end. */
function TextAsk() {
  let box: HTMLTextAreaElement | undefined;
  return (
    <>
      <textarea
        ref={box}
        rows={2}
        placeholder="is it cold in the workshop?"
        style={{ width: '100%', 'font-family': 'inherit', 'font-size': '.95rem' }}
      />
      <button
        onClick={() => {
          const v = box?.value ?? '';
          if (v.trim()) void askText(v, 'typed');
        }}
      >
        Ask
      </button>
    </>
  );
}

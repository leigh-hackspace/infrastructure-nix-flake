import { Show } from 'solid-js';
import { Card } from './Card';
import { LevelMeter } from './LevelMeter';
import {
  mode,
  onWakeClick,
  recording,
  WAKE_WORD_LABEL,
  wakeBusy,
  wakeResult,
  wakeStatus,
} from '../state/controller';
import { playAudio } from '../lib/format';

/**
 * Wake word: listens for the configured keyword(s) client-side (OpenWakeWord
 * WASM — the GTX 1060 is not touched while listening), then streams the
 * command to the gateway and auto-commits after a pause.
 */
export function WakeWordCard() {
  return (
    <Card title="Wake word">
      <p class="sub">
        Say “{WAKE_WORD_LABEL}” to start a transcription; it auto-commits after you pause.
      </p>
      <button class={`mic ${recording() ? 'on' : ''}`} disabled={wakeBusy()} onClick={onWakeClick}>
        {recording() ? '🔴 Disable wake word' : `💤 Enable wake word`}
      </button>
      <div class={`sub ${wakeStatus().kind}`}>{wakeStatus().text}</div>
      <div class={`result ${wakeResult().kind}`}>
        {wakeResult().text}
        <Show when={wakeResult().playUrl}>
          <button
            class="play"
            title="Play recorded audio"
            onClick={() => playAudio(wakeResult().playUrl!)}
          >
            ▶ play
          </button>
        </Show>
      </div>
      <LevelMeter visible={() => recording() && mode() === 'wake'} />
    </Card>
  );
}

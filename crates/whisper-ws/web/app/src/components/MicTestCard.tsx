import { Show } from 'solid-js';
import { Card } from './Card';
import { LevelMeter } from './LevelMeter';
import { mode, onMicClick, recording, testBusy, testResult, testStatus } from '../state/controller';
import { playAudio } from '../lib/format';

/**
 * WebSocket test: streams the mic as 24 kHz PCM16 over the Realtime WebSocket
 * and shows the transcript, proving the gateway end-to-end.
 */
export function MicTestCard() {
  return (
    <Card title="WebSocket test">
      <button class={`mic ${recording() ? 'on' : ''}`} disabled={testBusy()} onClick={onMicClick}>
        {recording() ? '🔴 Stop' : '🎤 Test mic'}
      </button>
      <div class={`sub ${testStatus().kind}`}>{testStatus().text}</div>
      <div class={`result ${testResult().kind}`}>
        {testResult().text}
        <Show when={testResult().playUrl}>
          <button
            class="play"
            title="Play recorded audio"
            onClick={() => playAudio(testResult().playUrl!)}
          >
            ▶ play
          </button>
        </Show>
      </div>
      <LevelMeter visible={() => recording() && mode() !== 'wake'} />
    </Card>
  );
}

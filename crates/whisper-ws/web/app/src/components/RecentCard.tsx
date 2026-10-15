import { For, Show } from 'solid-js';
import type { Accessor } from 'solid-js';
import { Card, Pill } from './Card';
import type { StatusSnapshot } from '../types';
import { ago, audioUrl, playAudio } from '../lib/format';

export function RecentCard(props: { snapshot: Accessor<StatusSnapshot | null> }) {
  return (
    <Card title="Recent transcriptions">
      <Show when={props.snapshot()}>
        {(snap) => (
          <>
            <ul class="transcripts">
              <For each={snap().recent}>
                {(r) => (
                  <li>
                    <Pill kind={r.ok ? 'good' : 'bad'}>{r.ok ? 'ok' : 'failed'}</Pill>{' '}
                    <span class="muted nowrap">{ago(r.completed_secs_ago)}</span>{' '}
                    <span class="muted nowrap">
                      {r.audio_secs.toFixed(1)}s audio / {(r.duration_ms / 1000).toFixed(1)}s
                    </span>
                    <Show when={r.language}>
                      <span class="muted">[{r.language}]</span>
                    </Show>
                    <br />
                    <code>{r.item_id}</code> {r.ok ? r.transcript : <span class="bad">{r.error || 'failed'}</span>}
                    <Show when={r.has_audio}>
                      <button
                        class="play"
                        title="Play recorded audio"
                        onClick={() => playAudio(audioUrl(r.session_id, r.item_id))}
                      >
                        ▶ play
                      </button>
                    </Show>
                  </li>
                )}
              </For>
              <Show when={snap().recent.length === 0}>
                <li class="muted">no transcriptions yet</li>
              </Show>
            </ul>
          </>
        )}
      </Show>
    </Card>
  );
}

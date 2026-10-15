import { For, Show } from 'solid-js';
import type { Accessor } from 'solid-js';
import { Card, Pill } from './Card';
import type { StatusSnapshot } from '../types';
import { ago } from '../lib/format';

export function WhisperCard(props: { snapshot: Accessor<StatusSnapshot | null> }) {
  return (
    <Card title="Whisper server">
      <Show when={props.snapshot()} fallback={<span class="muted">…</span>}>
        {(snap) => {
          const w = snap().whisper;
          return (
            <div>
              <Pill kind={w.ok ? 'good' : 'bad'}>{w.ok ? 'ok' : 'down'}</Pill>{' '}
              <span class="muted">
                {w.latency_ms}ms, checked {ago(w.checked_secs_ago)}
              </span>
              <Show when={w.error}>
                <span class="muted"> — {w.error}</span>
              </Show>
            </div>
          );
        }}
      </Show>
    </Card>
  );
}

export function TotalsCard(props: { snapshot: Accessor<StatusSnapshot | null> }) {
  const total = (label: string, value: Accessor<number | null>) => (
    <span>
      <span class="muted">{label}</span> <b>{value() ?? '…'}</b>
    </span>
  );

  return (
    <Card title="Totals">
      <div>
        {total('connections', () => props.snapshot()?.connections ?? null)}
        {' · '}
        {total('sessions', () => props.snapshot()?.sessions_total ?? null)}
        {' · '}
        {total('transcriptions', () => props.snapshot()?.transcriptions_total ?? null)}
        {' · '}
        {total('failed', () => props.snapshot()?.failed_total ?? null)}
      </div>
    </Card>
  );
}

export function SessionsCard(props: { snapshot: Accessor<StatusSnapshot | null> }) {
  return (
    <Card title="Active sessions">
      <table>
        <thead>
          <tr>
            <th>session</th>
            <th>connected</th>
            <th>buffered</th>
            <th>current item</th>
          </tr>
        </thead>
        <tbody>
          <Show when={props.snapshot()}>
            {(snap) => (
              <>
                <For each={snap().sessions}>
                  {(s) => (
                    <tr>
                      <td>
                        <code>{s.id}</code>
                      </td>
                      <td class="nowrap muted">{ago(s.connected_secs)}</td>
                      <td class="nowrap">
                        {s.buffered_secs.toFixed(1)}s <span class="muted">({ago(s.last_event_secs_ago)})</span>
                      </td>
                      <td>
                        <Show
                          when={s.current_item}
                          fallback={<span class="muted">—</span>}
                        >
                          <>
                            <Pill kind="wait">transcribing</Pill> <code>{s.current_item!.item_id}</code>{' '}
                            <span class="muted">
                              {s.current_item!.audio_secs.toFixed(1)}s audio,{' '}
                              {s.current_item!.elapsed_secs.toFixed(1)}s elapsed
                            </span>
                          </>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
                <Show when={snap().sessions.length === 0}>
                  <tr>
                    <td colspan={4} class="muted">
                      none connected
                    </td>
                  </tr>
                </Show>
              </>
            )}
          </Show>
        </tbody>
      </table>
    </Card>
  );
}

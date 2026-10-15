import { createSignal, onCleanup } from 'solid-js';
import type { Accessor } from 'solid-js';
import { MicTestCard } from './components/MicTestCard';
import { RecentCard } from './components/RecentCard';
import { SessionsCard, TotalsCard, WhisperCard } from './components/StatusCards';
import { WakeWordCard } from './components/WakeWordCard';
import { ago } from './lib/format';
import type { StatusSnapshot } from './types';

const POLL_MS = 1500;

export default function App() {
  const [snapshot, setSnapshot] = createSignal<StatusSnapshot | null>(null);
  const [offline, setOffline] = createSignal('');

  // The server is a static template + this poll — no SSE machinery needed
  // for a LAN dashboard.
  let timer = 0;
  const tick = () => {
    fetch('/status.json')
      .then((r) => r.json())
      .then((data: StatusSnapshot) => {
        setSnapshot(data);
        setOffline('');
      })
      .catch((e) => setOffline(String(e)))
      .finally(() => {
        timer = window.setTimeout(tick, POLL_MS);
      });
  };
  tick();
  onCleanup(() => window.clearTimeout(timer));

  const snap: Accessor<StatusSnapshot | null> = snapshot;

  return (
    <>
      <header>
        <h1>🎤 whisper-ws</h1>
        <span class={`pill ${offline() ? 'bad' : 'good'}`}>
          {offline() ? `offline: ${offline()}` : snapshot() ? `up ${ago(snapshot()!.uptime_secs)}` : '…'}
        </span>
        <span class="sub">{snapshot()?.whisper_url ?? ''}</span>
      </header>
      <main>
        <WhisperCard snapshot={snap} />
        <MicTestCard />
        <WakeWordCard />
        <TotalsCard snapshot={snap} />
        <SessionsCard snapshot={snap} />
        <RecentCard snapshot={snap} />
      </main>
    </>
  );
}

import { Show } from 'solid-js';
import { levelColor, levelDb, levelPeakDb, levelPct, nanSamples } from '../state/controller';

/**
 * Live input level meter: RMS of the mic chunk mapped to dBFS (green below
 * -30 dB, amber through -12 dB, red near clipping); the white tick holds the
 * recent peak so brief input is still visible.
 */
export function LevelMeter(props: { visible: () => boolean }) {
  const db = () => levelDb();
  const readout = () => {
    const d = db();
    return (
      <>
        {d < -99.5 ? (
          <>
            input — <span class="muted">no signal</span>
          </>
        ) : (
          <>
            input <b>{d.toFixed(1)}</b> dBFS{d > -6 && ' — very loud, watch for clipping'}
          </>
        )}
        {nanSamples() > 0 && (
          <span class="txt-err">
            {' · '}⚠ {nanSamples()} NaN samples from browser
          </span>
        )}
      </>
    );
  };

  return (
    <Show when={props.visible()}>
      <div class="level">
        <div class="meter">
          <div class="meter-fill" style={{ width: `${levelPct(db())}%`, background: levelColor(db()) }} />
          <div class="meter-tick" style={{ left: `${levelPct(levelPeakDb())}%` }} />
        </div>
        <div class="sub readout">{readout()}</div>
      </div>
    </Show>
  );
}

/**
 * The sound the page makes when it has heard you, and the mute that must come with it.
 *
 * Two halves, deliberately in one file. A confirmation cue with no mute is a microphone that
 * transcribes its own speaker — the assistant answers "yes?" to itself — and a mute with no cue
 * is a listener nobody can tell is listening. `crates/voice-assistant/README.md` states the same
 * pair as a *rule* on the device side (`satellite.rs`: capture during playback is discarded),
 * and the browser demo has always done it here; the reason it is exported now is that the
 * server-side wake path needed it too, and a second copy of a Web Audio blip is how two
 * detectors end up sounding different.
 *
 * No asset: an oscillator and a gain envelope, because an earcon that needs a fetch is an earcon
 * that fails offline, and this one has to fire the instant a detection lands.
 */

/** How long the mic ignores the room after the cue. Short enough that an immediate answer is not
 * lost, long enough to cover the cue's own tail (it stops at 0.32 s). */
export const CUE_MUTE_MS = 350;

/** Safe to call with no context (still starting, or suspended): a missing cue is a worse
 * failure than a silent one, so this never throws. */
export function playCue(ctx: AudioContext | null): void {
  if (!ctx || ctx.state !== 'running') return;
  const t = ctx.currentTime;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = 'sine';
  osc.frequency.setValueAtTime(880, t);
  osc.frequency.exponentialRampToValueAtTime(1320, t + 0.12);
  // Ramped from and to near-silence: a gain that starts or ends at a non-zero value is a click,
  // and a click in a room is a sound people hear as a fault.
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

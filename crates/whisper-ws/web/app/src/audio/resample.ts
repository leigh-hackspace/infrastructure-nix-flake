// Linear-interpolation resample from the AudioContext rate to an arbitrary
// target rate, with state carried across chunks. Used for the 24 kHz gateway
// stream and the 16 kHz wake-word tap.

export interface ResampleState {
  inBuf: number[];
  pos: number;
}

export function makeResampler(): ResampleState {
  return { inBuf: [], pos: 0 };
}

export function resetResampler(state: ResampleState): void {
  state.inBuf = [];
  state.pos = 0;
}

/** Resample one chunk to PCM16 sample values in [-32768, 32767]. */
export function resampleChunkTo(
  f32: Float32Array,
  state: ResampleState,
  sourceRate: number,
  targetRate: number,
): number[] {
  const n = state.inBuf.length;
  for (let j = 0; j < f32.length; j++) state.inBuf[n + j] = f32[j];
  const step = sourceRate / targetRate;
  const out: number[] = [];
  while (state.pos + 1 < state.inBuf.length) {
    const i0 = Math.floor(state.pos);
    const frac = state.pos - i0;
    const s = state.inBuf[i0] + (state.inBuf[i0 + 1] - state.inBuf[i0]) * frac;
    out.push(s < -1 ? -32768 : s > 1 ? 32767 : Math.round(s * 32767));
    state.pos += step;
  }
  const keep = Math.floor(state.pos);
  state.inBuf = state.inBuf.slice(keep);
  state.pos -= keep;
  return out;
}

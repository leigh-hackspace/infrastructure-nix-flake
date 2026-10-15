// Shared helpers.

/** "now" / "12s ago" / "3m ago" — same rules as the old status page. */
export function ago(secs: number): string {
  if (secs < 1) return 'now';
  if (secs < 60) return `${Math.round(secs)}s ago`;
  return `${Math.round(secs / 60)}m ago`;
}

/** Recorded audio for a completed item, as the 16 kHz mono WAV whisper-server heard. */
export function audioUrl(sessionId: string, itemId: string): string {
  return `/audio/${sessionId}/${itemId}.wav`;
}

export function playAudio(url: string): void {
  new Audio(url).play().catch((e) => {
    console.error('playback failed:', e);
  });
}

/**
 * Base64-encode a byte array, chunked so String.fromCharCode.apply doesn't
 * blow the argument stack on long clips.
 */
export function b64(bytes: Uint8Array): string {
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
}

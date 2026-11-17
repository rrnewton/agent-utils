// The silence before a chunk that starts cold (`#1800 vibe-talk-read-aloud-chunk-start-clipping`).
//
// A Bluetooth sink (a car's speakers, earbuds) that has gone idle between chunks drops the
// first few hundred milliseconds of the next thing it is given while it wakes. The page's
// answer is to hand the sink silence first: a chunk with no contiguous predecessor — the
// first of a read, or one whose predecessor ended before its audio arrived — is preceded by
// `readPrerollMs` of scheduled silence, which the sink may clip instead of speech. A chunk
// that begins exactly where its predecessor ends is gapless already and gets none, so
// wired and phone-speaker listening pays nothing for a fix it does not need.

/** The default pre-roll, in milliseconds: enough for the common car and headset sinks. */
const DEFAULT_READ_PREROLL_MS = 300;

/** How much silence precedes a chunk: the pre-roll when the start is cold, else none. */
function chunkStartSilenceMs(cold: boolean, prerollMs: number): number {
  return cold && prerollMs > 0 ? prerollMs : 0;
}

/** Whole frames of silence `ms` holds at `rate` samples a second. */
function silenceFrameCount(ms: number, rate: number): number {
  return Math.max(0, Math.round((ms / 1000) * rate));
}

export { DEFAULT_READ_PREROLL_MS, chunkStartSilenceMs, silenceFrameCount };

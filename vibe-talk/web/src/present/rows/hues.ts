// A thread's colour (`#624 voice-page-modularity`, moved from the legacy script into present/rows): how
// far apart hues stand, the re-roll that keeps a new thread clear of its neighbours (`#414
// thread-hue-spread`), and which remembered colours are kept (`#396 thread-color-memory`). Which
// threads are a new one's neighbours is the legacy script's to say, from what the channel holds.

// `#414 thread-hue-spread`. The widest arc is the colour farthest from every thread the channel has,
// but once `#396` remembers two dozen of them that arc can be 15° wide, and nothing kept the threads
// drawn either side of a new one out of it: three consecutive roots came out three shades of one
// colour. A new thread's colour must now stand clear of its neighbours, and is re-rolled if it does not.
/**
 * How far apart, in degrees of hue, a new thread's colour is kept from its neighbours'. 45° is one
 * step of the eight hue families the eye names (red, orange, yellow, green, cyan, blue, violet,
 * magenta) at the page's fixed 55% saturation and 50% lightness, so neighbours never share one: at
 * 30° yellow-green and green, or two blues, still read as one colour at a 3px edge. It is also as far
 * as can still be promised: eight neighbours spread 45° apart leave nothing that clears them all.
 */
const THREAD_HUE_APART = 45;

/**
 * How far, more strictly, from the thread drawn right before or after it, where it can be had. At 45°
 * three threads in a row can still step along the wheel — blue, violet, magenta — and read as a
 * gradient; a quarter turn puts blue beside magenta or green, which reads as unrelated.
 */
const THREAD_HUE_BESIDE = 90;

const hueApart = (a: number, b: number): number => {
  const apart = Math.abs(a - b) % 360;
  return Math.min(apart, 360 - apart);
};

/**
 * `widest`, the widest arc's middle, if it clears the neighbours: `THREAD_HUE_APART` from every colour
 * in `near` and `THREAD_HUE_BESIDE` from every one in `beside`. Else the re-roll: every whole degree is
 * a candidate, the farthest from every colour in `used` first and the lower hue on a tie, so the same
 * colours in use and neighbours always re-roll to the same colour, and the first that clears them wins. Where none does,
 * the same again asking only `THREAD_HUE_APART` of every neighbour; where none does that either, the
 * candidate farthest from its nearest neighbour.
 */
function spreadThreadHue(widest: number, used: readonly number[], { near, beside }: { readonly near: readonly number[]; readonly beside: readonly number[] }): number {
  const clearance = (hue: number, of: readonly number[]): number =>
    of.reduce((least, other) => Math.min(least, hueApart(hue, other)), 360);
  const tiers: readonly [(hue: number) => boolean, (hue: number) => boolean] = [
    (hue) => clearance(hue, near) >= THREAD_HUE_APART && clearance(hue, beside) >= THREAD_HUE_BESIDE,
    (hue) => clearance(hue, near) >= THREAD_HUE_APART,
  ];
  if (tiers[0](widest)) return widest;
  const spread = (hue: number): number => clearance(hue, used);
  const candidates = Array.from({ length: 360 }, (_, hue) => ({ hue, spread: spread(hue) }))
    .sort((a, b) => b.spread - a.spread || a.hue - b.hue);
  for (const clears of tiers) {
    if (clears(widest)) return widest;
    const first = candidates.find((candidate) => clears(candidate.hue));
    if (first) return first.hue;
  }
  let best = candidates[0] as { hue: number; spread: number };
  for (const candidate of candidates) if (clearance(candidate.hue, near) > clearance(best.hue, near)) best = candidate;
  return best.hue;
}

/**
 * The newest `limit` of `entries` ([id, [hue, lastDrawn]]), by when each was last drawn, as an
 * object. Built by `fromEntries`, so an id such as `__proto__` is a key like any other.
 */
function newestThreadHues(entries: ReadonlyArray<readonly unknown[]>, limit: number): Record<string, unknown> {
  // Each entry is [id, [hue, lastDrawn]], as the caller has checked; typed loosely so the legacy
  // script's untyped arrays can be handed over as they are.
  const hues = entries as ReadonlyArray<readonly [string, readonly [number, number]]>;
  return Object.fromEntries(hues.length <= limit ? hues
    : hues.slice().sort((a, b) => b[1][1] - a[1][1]).slice(0, limit));
}

export { THREAD_HUE_APART, THREAD_HUE_BESIDE, hueApart, newestThreadHues, spreadThreadHue };

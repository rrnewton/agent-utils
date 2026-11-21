// The render window over the channel's rows (`#1846 vibe-talk-windowed-rendering`, step 3):
// only the viewport plus a bounded prefetch margin is mounted in #discord-log, between two
// spacers whose heights come from the height index (step 2). Off-screen groups exist only as
// cache data (step 1); a redraw mounts its window and nothing else, so an off-screen update
// does no DOM work. Scrolling re-slices the window; the anchor row is held by one measured
// write through the scroll owner, never by the window manager itself.

/** The slice of groups to mount: [from, to) into the full group list. */
export interface WindowSlice {
  from: number;
  to: number;
}

/** Cumulative offsets for `heights`: offsets[i] is group i's top; last entry is the total. */
export function offsetsFor(heights: readonly number[]): number[] {
  const offsets: number[] = [0];
  for (const height of heights) offsets.push((offsets[offsets.length - 1] as number) + height);
  return offsets;
}

/**
 * The window for a scroll position: the visible band expanded by `prefetchPx` on both sides,
 * then held to at most `cap` groups (whichever bound hits first; the cap trims the far side
 * first, keeping the side the reader is moving toward less). Pure; unit-tested.
 */
export function sliceFor(
  offsets: readonly number[],
  scrollTop: number,
  viewPx: number,
  prefetchPx: number,
  cap: number,
): WindowSlice {
  const count = offsets.length - 1;
  if (count <= 0) return { from: 0, to: 0 };
  const total = offsets[count] as number;
  // `#2008`: the reader at or past the priced end (the height estimates running
  // short of the real layout, as a send lands): the window is the last `cap`
  // groups, never an empty range - an empty slice unmounts every row the
  // reader is looking at for that render (the send-flash A -> B -> A).
  if (scrollTop >= total) return { from: Math.max(0, count - cap), to: count };
  const top = Math.max(0, Math.min(scrollTop - prefetchPx, total));
  const bottom = Math.max(top, Math.min(scrollTop + viewPx + prefetchPx, total));
  let from = 0;
  while (from < count && (offsets[from + 1] as number) <= top) from += 1;
  let to = from;
  while (to < count && (offsets[to] as number) < bottom) to += 1;
  if (to - from > cap) {
    // Keep the visible band: find its first group and centre the cap on it.
    let visible = from;
    while (visible < count && (offsets[visible + 1] as number) <= scrollTop) visible += 1;
    from = Math.max(0, Math.min(visible - Math.floor(cap / 3), count - cap));
    to = Math.min(count, from + cap);
  }
  return { from, to };
}

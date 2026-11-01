// Read-aloud steering math, with no browser or page state (`#624 voice-page-modularity`).
import type { SpeechSegment } from "./pieces.ts";
import type { PreparedBlock } from "./prepared.ts";

export interface ReadTimeline {
  startsAt: number | null;
  endsAt: number;
  finished: boolean;
  gaps: readonly (readonly [number, number])[];
}


/**
 * `#376 read-aloud-blocks`. Where the parts of one read are scheduled on the audio clock: each part
 * begins where the one before it ends, so a block boundary adds no silence when the next block's
 * audio is already there. `playAt` is the context time the next sample is due.
 */
export function readClock() {
  return { playAt: 0 };
}


/**
 * How long a part's audio clock ran with nothing to play before `at`, from its first sample: the
 * underruns, which say nothing. `#472 gate-flake-hardening`: counted as speech, a stall in the
 * middle of a block moved the light on through the silence, then BACK a sentence once the block had
 * arrived whole and its length was the audio's own, gaps and all.
 */
export function silentBefore(timeline: ReadTimeline, at: number) {
  let silent = 0;
  for (const [from, to] of timeline.gaps || []) silent += Math.max(0, Math.min(to, at) - from);
  return silent;
}


/**
 * `read-chunk-balance`. How wide each block of the timeline is, from how long each says (`chars`): by
 * the square root, and none over twice the mean, so a long block reads as longer without taking the
 * line. The owner, 2026-10-27: one block must not take most of the width. Rounded to thousandths.
 *
 * @param {number[]} chars
 * @returns {number[]}
 */
export function readTimelineWeights(chars: number[]) {
  const roots = chars.map((count) => Math.sqrt(Math.max(1, count)));
  const mean = roots.reduce((sum, root) => sum + root, 0) / Math.max(1, roots.length);
  return roots.map((root) => Math.round(Math.min(root, 2 * mean) * 1000) / 1000);
}


/**
 * `#627 scrubber-thumb-arrows`. Where each SECTION of a read begins, as indices into its segments, in
 * order, the first always 0: the owner's "big jumps between chapters", beside the blocks' small ones.
 *
 * A section is a message of the row, or a heading of one and what follows it up to the next. A read
 * with neither (one message, no headings: most of them) would leave the fat arrows nothing to do, so
 * there the section is the message's next level down: a drawn block whole, its pieces
 * (`#486 read-aloud-pieces`) together and a run of list items together. Only the kinds and pieces the
 * blocks were prepared as are read, so a row scrolled away or drawn again steers the same.
 *
 * @param {{segments: Array<{part: number, block: number | null}>, blocks?: Array<Array<{kind?: string, piece?: number}> | null> | null}} playing
 * @returns {number[]}
 */
export function readSectionStarts(playing: { segments: readonly SpeechSegment[]; blocks?: (readonly PreparedBlock[] | null)[] | null }) {
  const { segments } = playing;
  const listedOf = (segment: SpeechSegment) => (segment.block === null ? null
    : (playing.blocks! && playing.blocks![segment.part]! && playing.blocks![segment.part]![segment.block!]!) || null);
  const continues = (listed: PreparedBlock | null) => listed !== null && Number.isInteger(listed.piece) && Number(listed.piece) > 0;
  const startsWhere = (begins: (listed: PreparedBlock | null, before: PreparedBlock | null, segment: SpeechSegment, previous: SpeechSegment) => boolean) => segments.flatMap((segment, index) =>
    (index === 0 || begins(listedOf(segment), listedOf(segments[index - 1]!), segment, segments[index - 1]!) ? [index] : []));
  const chapters = startsWhere((listed, _before, segment, previous) => segment.part !== previous.part
    || listed === null || (listed.kind === "heading" && !continues(listed)));
  if (chapters.length > 1) return chapters;
  return startsWhere((listed, before) => listed === null
    || (!continues(listed) && !(listed.kind === "item" && before !== null && before.kind === "item")));
}

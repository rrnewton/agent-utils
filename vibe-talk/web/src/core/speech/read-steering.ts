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


/**
 * `#2266 vibe-talk-doc-reader-navigator-campaign`. One drawn block's prepared
 * pieces together: consecutive listed blocks sharing one server `element`
 * (a table's rows, a long block's sentence pieces). A server from before
 * pieces lists one block per drawn block with no `element`; each is then its
 * own group, in order.
 */
export interface DocumentBlockGroup {
  readonly kind: string;
  readonly element: number | null;
  readonly members: number[];
}

/** The element groups of one prepared list, in order (see above). */
export function documentBlockGroups(listed: readonly PreparedBlock[]): DocumentBlockGroup[] {
  const groups: DocumentBlockGroup[] = [];
  listed.forEach((block, index) => {
    const element = block && Number.isInteger(block.element) ? block.element as number : null;
    const last = groups[groups.length - 1];
    if (last && element !== null && last.element === element) last.members.push(index);
    else groups.push({ kind: String(block && block.kind || ""), element, members: [index] });
  });
  return groups;
}

/**
 * Whether `groups` align onto a caller's block model: one group a model
 * block, kinds agreeing in order. No model (`kinds` absent or empty) never
 * aligns; callers then keep the prepared list's own numbering.
 */
export function documentGroupsAlign(groups: readonly DocumentBlockGroup[], kinds: readonly string[] | undefined): boolean {
  return !!kinds && kinds.length > 0 && groups.length === kinds.length
    && groups.every((group, at) => group.kind === kinds[at]);
}

function drawnElementOf(listed: readonly PreparedBlock[], listedIndex: number): number {
  const block = listed[listedIndex];
  return block && Number.isInteger(block.element) ? block.element as number : listedIndex;
}

/**
 * The caller's block-model index of prepared block `listedIndex`: its group
 * ordinal where the cuts align, else the server's drawn element (the
 * `#2004` fallback light), else the listed index itself where the caller
 * cut no model. The one mapping position reports use.
 */
export function documentModelBlockForListed(
    listed: readonly PreparedBlock[], kinds: readonly string[] | undefined, listedIndex: number): number {
  if (!kinds || kinds.length === 0) return listedIndex;
  const groups = documentBlockGroups(listed);
  if (documentGroupsAlign(groups, kinds)) {
    const at = groups.findIndex((group) => group.members.includes(listedIndex));
    if (at !== -1) return at;
  }
  return drawnElementOf(listed, listedIndex);
}

/** The above with the piece's place in its block: `{block, piece}` in model space. */
export function documentModelPieceForListed(
    listed: readonly PreparedBlock[], kinds: readonly string[] | undefined, listedIndex: number): { block: number; piece: number } {
  if (!kinds || kinds.length === 0) return { block: listedIndex, piece: 0 };
  const groups = documentBlockGroups(listed);
  const at = groups.findIndex((group) => group.members.includes(listedIndex));
  if (at === -1) return { block: drawnElementOf(listed, listedIndex), piece: 0 };
  const piece = Math.max(0, groups[at]!.members.indexOf(listedIndex));
  return documentGroupsAlign(groups, kinds)
    ? { block: at, piece }
    : { block: drawnElementOf(listed, listedIndex), piece };
}

/**
 * The prepared block a seek to model block `modelBlock` (its `piece`-th
 * piece, clamped) starts at, or -1 where the list is empty: the group's
 * member where the cuts align; else the group the server drew as that
 * block (nearest at or before it); with no model, the listed index itself,
 * clamped, as before a model was cut. Seeking in the page's block space is
 * the inverse of {@link documentModelBlockForListed}; nothing outside the
 * read owner ever names a listed index for a document.
 */
export function documentListedBlockForModel(
    listed: readonly PreparedBlock[], kinds: readonly string[] | undefined,
    modelBlock: number, piece = 0): number {
  if (listed.length === 0) return -1;
  const clamp = (value: number, last: number) => Math.max(0, Math.min(last, Math.floor(value) || 0));
  if (!kinds || kinds.length === 0) return clamp(modelBlock, listed.length - 1);
  const groups = documentBlockGroups(listed);
  let group: DocumentBlockGroup | undefined;
  if (documentGroupsAlign(groups, kinds)) {
    group = groups[clamp(modelBlock, groups.length - 1)];
  } else {
    for (const candidate of groups) {
      const drawn = candidate.element ?? groups.indexOf(candidate);
      if (drawn <= modelBlock) group = candidate;
      if (drawn === modelBlock) break;
      if (drawn > modelBlock && group) break;
    }
    group = group ?? groups[0];
  }
  if (!group || group.members.length === 0) return -1;
  return group.members[clamp(piece, group.members.length - 1)]!;
}

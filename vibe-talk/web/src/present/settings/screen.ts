// What Settings' Screen section says about the text size and the reading width, and how their keys and
// buttons move them (`#55 voice-desktop-app`, `#230 text-size-buttons`; `#624 voice-page-modularity`,
// moved from the legacy script into present/settings). Their bounds are web/src/core/settings's; the
// controls are web/src/view/settings's.

import { MAX_READING_CH, MIN_MSG_SCALE, MIN_READING_CH } from "../../core/settings/index.ts";

/** The text size slider's `step` in web/voice.html, and what one press of its − or + moves it by. */
const MSG_SCALE_STEP = 5;

/**
 * `#230 text-size-buttons`. Where one step down (-1) or up (+1) from the size `pct` lands: on one of the
 * slider's marks, and from a size between two (a hand-edited 83 in storage), the next mark that way, as
 * the slider's own `stepUp` and `stepDown` would.
 */
function msgScaleStepTarget(pct: number, direction: 1 | -1): number {
  const marks = (pct - MIN_MSG_SCALE) / MSG_SCALE_STEP;
  const mark = direction > 0 ? Math.floor(marks) + 1 : Math.ceil(marks) - 1;
  return MIN_MSG_SCALE + mark * MSG_SCALE_STEP;
}

/** What Settings says of a text size just chosen: kept, or refused by the browser and kept until a reload. */
const msgScaleWords = (pct: number, kept: boolean): string => kept
  ? `Saved — message text at ${pct}% of its usual size.`
  : "This browser refused to store the text size, so it will be forgotten when you reload " +
    `(private browsing does this). Message text is at ${pct}% until then.`;

/** What Settings says of a reading width just chosen: kept, or refused by the browser and kept until a reload. */
const readingWidthWords = (ch: number, kept: boolean): string => kept
  ? `Saved — a column ${ch} characters wide.`
  : "This browser refused to store the reading width, so it will be forgotten when you " +
    `reload (private browsing does this). The column is ${ch} characters wide until then.`;

/**
 * The width handle's keys, each the width it moves to from `width`. A separator with `tabindex` is only
 * a control if the arrow keys move it: Shift for a bigger step, Home/End for the ends, which is what a
 * range input does and therefore what a reader who has used one expects.
 */
const GRIP_KEYS: { readonly [key: string]: ((width: number) => number) | undefined } = {
  ArrowLeft: (width) => width - 1,
  ArrowRight: (width) => width + 1,
  ArrowDown: (width) => width - 1,
  ArrowUp: (width) => width + 1,
  PageDown: (width) => width - 10,
  PageUp: (width) => width + 10,
  Home: () => MIN_READING_CH,
  End: () => MAX_READING_CH,
};

export { GRIP_KEYS, MSG_SCALE_STEP, msgScaleStepTarget, msgScaleWords, readingWidthWords };

// The chips, the status pill and the read-aloud strip standing on the channel's box (`#245 done-feedback`,
// `#269 approval-card-inline`, `#433 read-strip-low`; `#624 voice-page-modularity`, moved from the legacy
// script into view/shell): where the box, or a draft waiting above it, reaches up into the strip they float
// in at the foot, they stand on its top edge instead, as a sticky row would; docked under another screen,
// on the draft's card. Written as `--composer-top`, how far above the foot of the list that edge is, which
// the stylesheet stands them on. Holding the read-aloud strip against where it now docks is the page's.

/** How far the status pill reaches above the chips, in rem: its gap and its least height (`#status-line`). */
const CHIPS_PILL_REM = 2.4;

/** What standing the chips asks of the page. */
export interface ChipsOverComposerDependencies {
  /** The page's element by id, in the left column (`leftEl`). */
  lookup(id: string): HTMLElement;
  /** The page's element by id, in the column being worked on (`el`). */
  el(id: string): HTMLElement;
  /** Whether the channel is what is on screen: the main screen, on the channel's view. */
  onChannel(): boolean;
  /** The root element's font size, in px (`rootFontPx`). */
  rootFontPx(): number;
  /** The root element. */
  root(): HTMLElement;
  /** The band the read-aloud strip floats in moved: hold it inside the window (`placeReadSteer`). */
  placeReadSteer(): void;
}

/** Standing the chips' one command. */
export interface ChipsOverComposer {
  /** Measure where the box, or the draft over it, reaches into the chips' strip, and stand them on it. */
  placeChipsOverComposer(): void;
}

/** Standing at the foot. */
export function createChipsOverComposer(deps: ChipsOverComposerDependencies): ChipsOverComposer {
  const leftEl = deps.lookup;
  let composerTop = 0;

  function placeChipsOverComposer(): void {
    let top = 0;
    // `#269 approval-card-inline`. A draft waiting above the channel's box is part of what they stand
    // on: they float over the messages above it, never over its Send. Docked under another screen, the
    // pill stands on the card in the same way, and under the thread list too (`#281 view-keys`), where
    // the channel's box is behind the list and out of sight.
    const card = leftEl("post-confirm");
    const carded = !card.hidden && card.parentNode === leftEl("pane-discord");
    const docked = !card.hidden && card.getAttribute("data-place") === "dock";
    if (deps.onChannel() && !docked) {
      const area = leftEl("scroll-area");
      const box = leftEl("channel-composer");
      const below = area.scrollHeight - area.scrollTop - (area.clientHeight || 0);
      const lowest = (box.offsetHeight || 0) + (carded ? card.offsetHeight || 0 : 0);
      if (below < (area.clientHeight || 0) + lowest) {
        const foot = leftEl("screen-main").getBoundingClientRect().bottom;
        const edge = box.getBoundingClientRect();
        const cardEdge = carded ? card.getBoundingClientRect() : null;
        // The strip they float in at the foot: half a rem off it, the chips, and the pill over them.
        // `#433 read-strip-low`: and the read-aloud strip between them while a read is up.
        const steer = deps.el("read-steer");
        const strip = deps.rootFontPx() * (0.5 + CHIPS_PILL_REM) + leftEl("scroll-tools").getBoundingClientRect().height +
          (steer && !steer.hidden ? steer.getBoundingClientRect().height + deps.rootFontPx() * 0.5 : 0);
        const reaches = edge.bottom > foot - strip || (cardEdge !== null && cardEdge.bottom > foot - strip);
        if (reaches) top = Math.max(0, Math.round(foot - (cardEdge ? cardEdge.top : edge.top)));
      }
    } else if (docked) {
      top = Math.max(0, Math.round(leftEl("frame-body").getBoundingClientRect().bottom - card.getBoundingClientRect().top));
      // `scrubber-full-width`. A phone's strip playing is a row at the screen's foot, over the card:
      // what stands on the card stands on the strip as well (the stylesheet's floor for the strip is
      // the larger of the two, not their sum).
      const steer = deps.el("read-steer");
      if (steer && !steer.hidden && deps.root().getAttribute("data-read-strip") !== null &&
          steer.getBoundingClientRect().bottom >= leftEl("screen-main").getBoundingClientRect().bottom - 1) {
        top += Math.round(steer.getBoundingClientRect().height);
      }
    }
    if (top === composerTop) return;
    composerTop = top;
    leftEl("frame-body").style.setProperty("--composer-top", `${top}px`);
    // `#451 read-strip-followups`: the band the strip floats in just moved with the box, so a strip
    // put somewhere on a desk is held inside the window against where it now docks.
    deps.placeReadSteer();
  }

  return { placeChipsOverComposer };
}

// What the page and the tests use of it.
export { CHIPS_PILL_REM };

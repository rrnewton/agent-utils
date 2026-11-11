// The gap a packed thread leaves, held and then closed (`#460 pack-gap-slide`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): the slides running, the card and its rows drawn over what
// slides, ending them at once, what an item of the cards is across a redraw, the items after a card's
// thread, and the slide itself — everything after the thread on the screen drawn where it was, held a
// moment when packed, and let slide home, the thread's rows fading in when unpacked. Holding the card still
// is the page's; why the motion is a FLIP over the settled layout is told where the legacy script opens the
// section.

import { rowShown } from "./rows.ts";

/** What the slides ask of the page. */
export interface PackSlidesDependencies {
  /** The cards' list (`side-cards`). */
  cards(): HTMLElement;
  /** The right column's list holding the cards and what follows them (`side-list`). */
  list(): HTMLElement;
  /** The right column's scroller (`side-scroll`). */
  area(): HTMLElement;
  /** A duration the stylesheet says on #screen-main, in ms: 0 where it says none (`--pack-slide`, `--pack-gap-hold`). */
  motionMs(name: string): number;
  /** Whether a reply is open under a row, whose box is anchored to it: then nothing slides. */
  replyDocked(): boolean;
  /** Whether the page is hidden: then nothing slides. */
  hidden(): boolean;
}

/** The slides' commands and questions. */
export interface PackSlides {
  /** End every slide at once: everything drawn where it is laid out. */
  finishPackSlide(): void;
  /** What an item of the cards is across a redraw, which draws every item anew: its kind and its id. */
  slideKey(item: HTMLElement): string;
  /** The first item drawn after card `row`'s thread, or null. */
  itemAfterBlock(row: HTMLElement): HTMLElement | null;
  /**
   * Card `row` was just packed or unpacked and held still, and the first item after its thread stood at
   * `was.top` before: draw everything after the thread on the screen where it was, hold it there a moment
   * (packed), and let it slide home.
   */
  slideIntoPlace(row: HTMLElement, was: { key: string; top: number }): void;
}

/** Nothing sliding. */
export function createPackSlides(deps: PackSlidesDependencies): PackSlides {
  /** The slides running, to end them (`finishPackSlide`). */
  const packSlides = new Set<Animation>();

  /**
   * The card a slide is for and its own rows, drawn over what slides (`data-slide-over`): what is below
   * starts where it was, over the room the card or its rows just took, and would cover them, and take a
   * press meant for them, until it had slid away.
   */
  const slideOver = new Set<HTMLElement>();

  /** The slides ended or done: the card and its rows drawn in the list's order again. */
  function endSlideOver(): void {
    for (const item of slideOver) item.removeAttribute("data-slide-over");
    slideOver.clear();
  }

  function finishPackSlide(): void {
    for (const motion of [...packSlides]) motion.finish();
    packSlides.clear();
    endSlideOver();
  }

  function slideKey(item: HTMLElement): string {
    return JSON.stringify([item.tagName, item.id || "", item.hasAttribute("data-card"), item.getAttribute("data-unpack-thread") || "",
      item.getAttribute("data-id") || item.getAttribute("data-ids") || ""]);
  }

  /**
   * The items drawn after card `row`'s thread, in order: the cards' items after its unpacked rows, then
   * what follows the cards in their list (Main's box, `#419 threads-view-main-messages`). Only those shown.
   */
  function itemsAfterBlock(row: HTMLElement): HTMLElement[] {
    const id = row.getAttribute("data-id");
    const cards = deps.cards();
    const items = [...cards.children] as HTMLElement[];
    const after = items.slice(items.indexOf(row) + 1).filter((item) => item.getAttribute("data-unpack-thread") !== id);
    const below = ([...deps.list().children] as HTMLElement[]).slice([...deps.list().children].indexOf(cards) + 1);
    return [...after, ...below].filter((item) => rowShown(item) &&
      (typeof item.getClientRects !== "function" || item.getClientRects().length > 0));
  }

  function itemAfterBlock(row: HTMLElement): HTMLElement | null {
    return itemsAfterBlock(row)[0] || null;
  }

  function slideIntoPlace(row: HTMLElement, was: { key: string; top: number }): void {
    const slideMs = deps.motionMs("--pack-slide");
    if (slideMs <= 0 || deps.replyDocked() || deps.hidden()) return;
    const items = itemsAfterBlock(row);
    const first = items[0];
    if (!first || typeof first.animate !== "function" || slideKey(first) !== was.key) return;
    const area = deps.area();
    const stage = area.getBoundingClientRect();
    const height = area.clientHeight || stage.height;
    const moved = was.top - first.getBoundingClientRect().top;
    if (Math.abs(moved) < 1) return;
    const shift = Math.sign(moved) * Math.min(Math.abs(moved), height);
    // Packed, the gap holds first; unpacked, the room opens at once.
    const holdMs = moved > 0 ? deps.motionMs("--pack-gap-hold") : 0;
    const total = holdMs + slideMs;
    const ease = "cubic-bezier(0.2, 0, 0, 1)";
    const from = `translateY(${shift}px)`;
    const frames: Keyframe[] = holdMs > 0
      ? [{ transform: from }, { transform: from, offset: holdMs / total, easing: ease }, { transform: "none" }]
      : [{ transform: from, easing: ease }, { transform: "none" }];
    const play = (item: HTMLElement, keyframes: Keyframe[]): void => {
      const motion = item.animate(keyframes, { duration: total });
      packSlides.add(motion);
      const done = (): void => {
        packSlides.delete(motion);
        if (packSlides.size === 0) endSlideOver();
      };
      motion.addEventListener("finish", done);
      motion.addEventListener("cancel", done);
    };
    const id = row.getAttribute("data-id");
    for (const item of [row, ...([...deps.cards().children] as HTMLElement[])
      .filter((one) => one.getAttribute("data-unpack-thread") === id)]) {
      item.setAttribute("data-slide-over", "");
      slideOver.add(item);
    }
    for (const item of items) {
      const box = item.getBoundingClientRect();
      // On the screen where it is drawn from, or where it comes to: the rest neither seen nor moved.
      if (Math.min(box.top, box.top + shift) >= stage.bottom) break;
      if (Math.max(box.bottom, box.bottom + shift) <= stage.top) continue;
      play(item, frames);
    }
    // Unpacked, the thread's own rows fade in as the cards slide down from under them.
    if (moved < 0) {
      for (const item of [...deps.cards().children] as HTMLElement[]) {
        if (item.getAttribute("data-unpack-thread") !== id || !rowShown(item)) continue;
        const box = item.getBoundingClientRect();
        if (box.bottom <= stage.top || box.top >= stage.bottom) continue;
        play(item, [{ opacity: 0, easing: ease }, { opacity: 1 }]);
      }
    }
  }

  return { finishPackSlide, slideKey, itemAfterBlock, slideIntoPlace };
}

/** A duration the stylesheet says on #screen-main, in ms: 0 where it says none (`cardFlightMs`). */
export function screenMotionMs(name: string): number {
  const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(document.getElementById("screen-main") as HTMLElement) : null;
  const said = style ? String(style.getPropertyValue(name)).trim() : "";
  const parsed = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(said);
  return parsed ? Number(parsed[1]) * (parsed[2] === "s" ? 1000 : 1) : 0;
}

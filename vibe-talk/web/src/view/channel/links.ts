// The links between Main's roots and their cards on a desk's two columns, as the DOM draws them
// (`#624 voice-page-modularity`, moved from the legacy script into view/channel; `#261
// two-column-connectors`): where a link meets a row, the order of the links across the gap, a path
// written only where it changed, and whether the mouse is on a node.

import { threadOf } from "../../core/channels/index.ts";
import { rowMessages, rowShown } from "./rows.ts";

/** What one path of a link is drawn from (the legacy script's `planColumnLinks` plans them). */
interface DrawnLink {
  readonly thread: string;
  readonly clip: string;
  readonly selected: boolean;
  readonly dimmed: boolean;
  readonly hue: number;
}

/** How far below a row's top its link leaves it, at most, when the row draws no tag to be level with, in px. */
const LINK_ROW_INSET_PX = 18;

/** The thread whose root `row` draws on Main as itself, or null: not a row of a collapsed run. */
function linkedRootThread(row: Element | null | undefined): string | null {
  if (!row || row.hasAttribute("data-read-run") || !rowShown(row)) return null;
  const first = rowMessages(row)[0];
  return first && first.thread && first.thread.is_root ? threadOf(first) : null;
}

/**
 * Where a link meets Main's row `row`, whose box is `box`: level with the middle of its Thread(N), the
 * tag in the thread's colour, or a little below its top when it draws none.
 */
function rowLinkY(row: HTMLElement & { threadBadge?: HTMLElement }, box: DOMRect): number {
  const badge = row.threadBadge;
  if (badge && badge.parentNode) {
    const tag = badge.getBoundingClientRect();
    if (tag.height > 0 && tag.top >= box.top && tag.bottom <= box.bottom) return (tag.top + tag.bottom) / 2;
  }
  return box.top + Math.min(box.height / 2, LINK_ROW_INSET_PX);
}

/**
 * The order of `links` across the gap, left to right, that crosses the fewest of them: the links rising
 * from their root to their card first, the higher root further left, then the falling ones, the higher
 * root further right. Of two links whose stretches of the gap overlap, both rising or both falling and
 * neither inside the other, that is the order in which neither crosses the other; a rising and a
 * falling one, or one inside the other, cross once whatever the order.
 */
function laneOrder<L extends { readonly from: { readonly y: number }; readonly to: { readonly y: number } }>(links: readonly L[]): L[] {
  const rising = links.filter((link) => link.to.y < link.from.y).sort((a, b) => a.from.y - b.from.y);
  const falling = links.filter((link) => link.to.y >= link.from.y).sort((a, b) => b.from.y - a.from.y);
  return [...rising, ...falling];
}

/** A coordinate for a path, to the half pixel, measured from `origin`. */
const linkAt = (value: number, origin: number): number => Math.round((value - origin) * 2) / 2;

/**
 * One path of a link, written only where it changed.
 */
function drawLinkPath(path: SVGElement, kind: string, link: DrawnLink, d: string): void {
  const attributes: Array<[string, string]> = [["class", kind], ["d", d], ["data-thread", link.thread], ["data-link", link.clip]];
  for (const [name, value] of attributes) {
    if (path.getAttribute(name) !== value) path.setAttribute(name, value);
  }
  if (path.hasAttribute("data-selected") !== link.selected) {
    if (link.selected) path.setAttribute("data-selected", "true");
    else path.removeAttribute("data-selected");
  }
  // Grey beside a highlighted card. `#277 thread-card-focus`.
  if (path.hasAttribute("data-dimmed") !== link.dimmed) {
    if (link.dimmed) path.setAttribute("data-dimmed", "true");
    else path.removeAttribute("data-dimmed");
  }
  if (path.style.getPropertyValue("--thread-hue") !== String(link.hue)) path.style.setProperty("--thread-hue", String(link.hue));
}

/**
 * Whether the mouse is on `node` now, as the browser says by `:hover`, which it keeps right as the
 * pointer moves, as the list scrolls under it and as it leaves the window. A browser that cannot say
 * counts as no.
 */
function pointerOn(node: Element | null | undefined): boolean {
  if (!node || typeof node.matches !== "function") return false;
  try {
    return node.matches(":hover");
  } catch (_error) {
    return false;
  }
}

export { LINK_ROW_INSET_PX, drawLinkPath, laneOrder, linkAt, linkedRootThread, pointerOn, rowLinkY };
export type { DrawnLink };

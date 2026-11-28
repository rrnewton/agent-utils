// The boxes for an open thread's replies on their way (`#2385 vibe-talk-thread-open-layout-first`;
// the first slice of the sparse incremental loading design, thread view only). What to draw is
// core/channels' (`planPendingReplies`); this owner draws it after the thread's rows, in a list of
// its own (`#pending-replies`, and the right column's twin), and takes it away when the read lands.
//
// - BOXES FROM THE FIRST PACKETS. The root's exact reply count comes with Main's read, before any
//   reply's body; the replies it reports and the store has not brought (`unseenReplies`) each get a
//   box at once, priced from the thread's replies on screen, while the read that brings them is on
//   its way. One line in the first box says how many ("8 more replies loading"); a screen reader is
//   told it ONCE, by the container's own status line, never once per box.
// - THE COUNT ARRIVING LATE. A thread drawn from this device's saved copy before Main's read landed
//   reads nothing at entry: nothing then said it was short. When a later read raises the open
//   thread's count above what is held, the thread is read at once and its boxes drawn
//   (`sync({ read: true })`), instead of one reply standing until the next poll.
// - NO Y-AXIS DISRUPTION (owner rule 370). The boxes stand below the thread's rows, so drawing
//   them moves nothing above; an entry with nothing unread among the held rows lands on the first
//   box as on a first unread reply (threadentry.ts), and the read keeps that landing on the reply
//   that takes the box's place (`keepEntryLanding`, by `heldBefore`).
// - EACH BOX PRICED FROM ITS OWN REPLY (S-t1). As the boxes are drawn, and before the thread's
//   bodies are read, the thread's layout is asked of the server (`readLayout`: each reply's id and
//   the size of its stored body, from the server's own copy). When it lands, the boxes are priced
//   again one per reply (`planLayoutReplies`) in ONE anchored transaction (`preservingScroll`):
//   the boxes stand below the rows, so a new height moves nothing above them, and a box set that
//   comes out shorter is held at least as tall as the scroll position needs (`reprice`), so the
//   list can never be clamped up under the reader. The bodies then fill the boxes as before.

import { planLayoutReplies, planPendingReplies } from "../../core/channels/index.ts";
import type { LayoutReply, PendingRepliesPlan } from "../../core/channels/index.ts";

/** A thread's layout as the boxes read it: the server's `ThreadLayoutResponse`, its entries in thread order. */
export interface PendingRepliesLayout {
  readonly entries: readonly LayoutReply[];
}

/** What drawing the boxes asks of the page. */
export interface PendingRepliesDependencies {
  /** The working column's element by id (`el`): `pending-replies`, `discord-log`. */
  el(id: string): HTMLElement | null;
  /** The boxes' kill switch: on unless the device keeps it off (`DEVICE_SETTINGS.threadReplyBoxes`). */
  enabled(): boolean;
  /** The thread whose list is on screen in the working column, or null (another view, Pinned up). */
  openThread(): string | null;
  /** How many replies thread `id`'s root reports that the store has not brought (`unseenReplies`). */
  missingReplies(id: string): number;
  /** Whether a read that brings the open thread's replies is on its way (`entryState().readOwed`). */
  readComing(): boolean;
  /** Owe the open thread its read, as entering it short does (`entryState().readOwed`). */
  oweRead(): void;
  /** Read the open thread now, quietly, behind its rows. */
  readThread(): void;
  /** Whether `row` is the open thread's root (`isOpenThreadRoot`): not a reply, so not a price for one. */
  isThreadRoot(row: HTMLElement): boolean;
  /**
   * Thread `id`'s layout from the server (`GET .../threads/{id}/layout`), or null where it has none
   * to give; asked before the thread's bodies are read. Absent: the boxes keep the count's price.
   */
  readLayout?(id: string): Promise<PendingRepliesLayout | null>;
  /** The ids of the messages the store holds (a reply drawn, or about to be). */
  heldIds?(): ReadonlySet<string>;
  /** Mutate the list holding the reader's row in place (`preservingScroll`, cause "other"). */
  preservingScroll?(mutate: () => void): void;
  /** The working column's scroller (`#scroll-area`), for the foot a re-price must not clamp. */
  scroller?(): HTMLElement | null;
  /** `act`, bound to the column it is made in, for a step that runs after a wait (`inThisColumn`). */
  keepColumn?(act: () => void): () => void;
}

/** The boxes' commands. */
export interface PendingReplies {
  /**
   * Draw the boxes the open thread is owed, or take them away. `read`: a count may have just
   * risen with no read of the thread on its way (a read of another view landed): read it now.
   */
  sync(options?: { read?: boolean }): void;
  /** The first box, while boxes stand in the working column; else null. */
  firstBox(): HTMLElement | null;
  /** Whether `node` is one of the boxes. */
  isBox(node: Element | null | undefined): boolean;
  /** The message id of the last row drawn above the boxes: where the first reply the read brings lands after. */
  heldBefore(): string | null;
}

/** Make the boxes' owner. */
/** How many of the thread's reply rows, nearest the boxes, a box is priced from. */
const REPLY_SAMPLE = 8;

export function createPendingReplies(deps: PendingRepliesDependencies): PendingReplies {
  function container(): HTMLElement | null {
    return deps.el("pending-replies");
  }

  /** Whether element `node` carries class `name` (by `className`: the page's plain DOM surface). */
  function hasClass(node: Element | null | undefined, name: string): boolean {
    return Boolean(node) && String((node as Element).className || "").split(/\s+/).includes(name);
  }

  /** The child of `box` carrying class `name`, or null. */
  function childOf(box: HTMLElement, name: string): HTMLElement | null {
    return ([...box.children] as HTMLElement[]).find((kid) => hasClass(kid, name)) || null;
  }

  /** The container's status line: told the box set's words once, and nothing per box. */
  function said(box: HTMLElement): HTMLElement {
    let line = childOf(box, "pending-replies-said");
    if (!line) {
      line = document.createElement("p");
      line.className = "pending-replies-said sr-only";
      line.setAttribute("role", "status");
      line.setAttribute("aria-live", "polite");
      box.append(line);
    }
    return line;
  }

  function listOf(box: HTMLElement): HTMLElement | null {
    return childOf(box, "pending-replies-list");
  }

  /**
   * The thread's last few reply rows drawn now, nearest the boxes: what a box is priced and shaped
   * from. A bounded sample, read from the list's end, so pricing the boxes reads a fixed number of
   * layouts however long the list is (`#1920`: a view switch reads O(1) layouts, never one per row).
   */
  function replyRows(): HTMLElement[] {
    const log = deps.el("discord-log");
    if (!log) return [];
    const rows = log.children;
    const sample: HTMLElement[] = [];
    for (let index = rows.length - 1; index >= 0 && sample.length < REPLY_SAMPLE; index--) {
      const row = rows[index] as HTMLElement;
      if (!row.hidden && row.getAttribute("data-id") && !deps.isThreadRoot(row)) sample.push(row);
    }
    return sample;
  }

  /** The heights of the thread's reply rows drawn now, for pricing a box. */
  function replyHeights(): number[] {
    return replyRows().map((row) => row.getBoundingClientRect().height).filter((height) => height > 0);
  }

  /**
   * A box takes the sides and corners of the thread's rows as drawn: a phone's rows run edge to edge
   * past the list's padding, a desk's sit inside it with round corners. Read from a reply row (else
   * any message row), so the box stands exactly where the reply will.
   */
  function shapeOf(): { left: string; right: string; radius: string } | null {
    const log = deps.el("discord-log");
    const row = replyRows()[0] || (log ? ([...log.children] as HTMLElement[]).find((one) => !one.hidden && one.getAttribute("data-id")) : null);
    if (!row || typeof getComputedStyle !== "function") return null;
    const style = getComputedStyle(row);
    return { left: style.marginLeft, right: style.marginRight, radius: style.borderRadius };
  }

  function clear(box: HTMLElement): void {
    if (!listOf(box) && !box.getAttribute("data-count")) return;
    const line = childOf(box, "pending-replies-said");
    if (line) line.textContent = "";
    box.replaceChildren(...(line ? [line] : []));
    box.removeAttribute("data-count");
    box.removeAttribute("data-px");
    box.removeAttribute("data-priced");
    box.style.minHeight = "";
  }

  /** The plan's heights in one word, to tell a re-price from the box set standing. */
  function pricedAs(plan: PendingRepliesPlan): string {
    return plan.heights ? `layout:${plan.heights.join(",")}` : "count";
  }

  /** Whether the boxes standing in `box` are `plan`'s already. */
  function drawnAs(box: HTMLElement, plan: PendingRepliesPlan): boolean {
    return box.getAttribute("data-count") === String(plan.count) && box.getAttribute("data-px") === String(plan.px) &&
      box.getAttribute("data-priced") === pricedAs(plan) && listOf(box) !== null;
  }

  function draw(box: HTMLElement, plan: PendingRepliesPlan): void {
    if (drawnAs(box, plan)) return;
    const shape = shapeOf();
    const list = document.createElement("ol");
    list.className = "messages pending-replies-list";
    list.setAttribute("aria-busy", "true");
    list.setAttribute("aria-label", plan.line);
    for (let index = 0; index < plan.count; index++) {
      const item = document.createElement("li");
      item.className = "pending-reply";
      item.setAttribute("aria-busy", "true");
      item.setAttribute("data-pending-reply", String(index));
      const px = plan.heights ? (plan.heights[index] ?? plan.px) : plan.px;
      item.setAttribute("data-px", String(px));
      item.style.height = `${px}px`;
      if (shape) {
        item.style.marginLeft = shape.left;
        item.style.marginRight = shape.right;
        item.style.borderRadius = shape.radius;
      }
      if (index === 0) {
        const words = document.createElement("span");
        words.className = "pending-replies-line";
        words.textContent = plan.line;
        item.append(words);
      }
      list.append(item);
    }
    const line = said(box);
    box.replaceChildren(line, list);
    const announced = box.getAttribute("data-count") !== String(plan.count);
    box.setAttribute("data-count", String(plan.count));
    box.setAttribute("data-px", String(plan.px));
    box.setAttribute("data-priced", pricedAs(plan));
    // Told once per box set: a re-price at the same count says nothing again.
    if (announced) line.textContent = plan.line;
  }

  /**
   * The thread's layout as last answered, for the thread it was asked for; and the thread and
   * shortfall it was last asked at, so one box set asks once and a new count asks again.
   */
  let layout: { thread: string; entries: readonly LayoutReply[] } | null = null;
  let layoutAsked: string | null = null;

  /** The box set for `thread`'s `missing` replies: priced one per reply when its layout has landed. */
  function planFor(thread: string, missing: number, readComing: boolean): PendingRepliesPlan | null {
    const measured = replyHeights();
    const counted = planPendingReplies(missing, readComing, true, measured);
    if (!counted || !layout || layout.thread !== thread || !deps.heldIds) return counted;
    const held = deps.heldIds();
    return planLayoutReplies(layout.entries, (id) => held.has(id), missing, counted.px) || counted;
  }

  /**
   * Draw `plan` in place of the boxes standing, held as one anchored change: nothing above the
   * boxes moves (they stand below the rows), and a box set that comes out shorter keeps the
   * container as tall as the scroll position needs, so the browser never clamps the list up under
   * the reader (owner rule 370). The hold goes with the boxes (`clear`).
   */
  function reprice(box: HTMLElement, plan: PendingRepliesPlan): void {
    const area = deps.scroller ? deps.scroller() : null;
    const before = box.getBoundingClientRect().height;
    const below = area ? area.scrollHeight - area.clientHeight - area.scrollTop : 0;
    const change = (): void => {
      draw(box, plan);
      const after = box.getBoundingClientRect().height;
      const floor = Math.ceil(before - Math.max(0, below));
      if (area && after < floor) box.style.minHeight = `${floor}px`;
    };
    if (deps.preservingScroll) deps.preservingScroll(change);
    else change();
  }

  /** Ask the server for `thread`'s layout once per shortfall, and price the boxes by it when it lands. */
  function askLayout(thread: string, missing: number): void {
    const key = `${thread}|${missing}`;
    if (!deps.readLayout || layoutAsked === key) return;
    layoutAsked = key;
    let answered: PendingRepliesLayout | null = null;
    // In the column that asked: the answer lands in a later task, which starts in the left one.
    const landed = (): void => {
      if (!answered || !Array.isArray(answered.entries) || !answered.entries.length) return;
      layout = { thread, entries: answered.entries };
      // Only boxes still standing for the same thread are priced again: the bodies may have landed
      // first, or the reader moved on, and then there is nothing to price.
      const box = container();
      if (!box || !listOf(box) || deps.openThread() !== thread || !deps.readComing() || !deps.enabled()) return;
      const plan = planFor(thread, deps.missingReplies(thread), true);
      if (plan && !drawnAs(box, plan)) reprice(box, plan);
    };
    const inColumn = deps.keepColumn ? deps.keepColumn(landed) : landed;
    deps.readLayout(thread).then((answer) => {
      answered = answer;
      inColumn();
    }).catch(() => { /* no layout: the boxes keep the count's price */ });
  }

  /**
   * The open thread and shortfall a read was last started for (`sync({ read: true })`): the same
   * shortfall after that read landed — the server's copy had no more yet — starts no second read,
   * so a lagging copy can never make reads chase each other. A new count starts one again.
   */
  let asked: string | null = null;

  function sync(options: { read?: boolean } = {}): void {
    const box = container();
    if (!box) return;
    const thread = deps.openThread();
    const missing = thread !== null && deps.enabled() ? deps.missingReplies(thread) : 0;
    if (missing > 0 && !deps.readComing() && options.read && asked !== `${thread}|${missing}`) {
      asked = `${thread}|${missing}`;
      deps.oweRead();
      // The layout goes out before the bodies' read, so it can price the boxes before they fill.
      askLayout(thread as string, missing);
      deps.readThread();
    }
    // Nothing to draw is decided before anything is measured: a sync on every view switch and read
    // landing must cost no layout reads unless boxes really stand.
    const readComing = deps.readComing();
    if (thread === null || missing <= 0 || !readComing || !deps.enabled()) {
      clear(box);
      return;
    }
    const plan = planFor(thread, missing, readComing);
    if (!plan) {
      clear(box);
      return;
    }
    // Boxes standing are priced again in place, held; a first box set is drawn below the rows.
    if (listOf(box) && !drawnAs(box, plan)) reprice(box, plan);
    else draw(box, plan);
    // Asked as the boxes first stand, before the read that brings the bodies is sent.
    askLayout(thread, missing);
  }

  function firstBox(): HTMLElement | null {
    const box = container();
    const list = box ? listOf(box) : null;
    return list ? ((list.children[0] as HTMLElement | undefined) || null) : null;
  }

  function isBox(node: Element | null | undefined): boolean {
    return hasClass(node, "pending-reply");
  }

  function heldBefore(): string | null {
    const log = deps.el("discord-log");
    if (!log) return null;
    const rows = [...log.children] as HTMLElement[];
    for (let index = rows.length - 1; index >= 0; index--) {
      const row = rows[index] as HTMLElement;
      const ids = String(row.getAttribute("data-ids") || row.getAttribute("data-id") || "").split(" ").filter(Boolean);
      if (ids.length && !row.hidden) return ids[ids.length - 1] as string;
    }
    return null;
  }

  return { sync, firstBox, isBox, heldBefore };
}

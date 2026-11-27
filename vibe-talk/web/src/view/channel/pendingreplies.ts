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

import { planPendingReplies } from "../../core/channels/index.ts";
import type { PendingRepliesPlan } from "../../core/channels/index.ts";

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
  }

  function draw(box: HTMLElement, plan: PendingRepliesPlan): void {
    if (box.getAttribute("data-count") === String(plan.count) && box.getAttribute("data-px") === String(plan.px) && listOf(box)) return;
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
      item.style.height = `${plan.px}px`;
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
    // Told once per box set: a re-price at the same count says nothing again.
    if (announced) line.textContent = plan.line;
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
      deps.readThread();
    }
    // Nothing to draw is decided before anything is measured: a sync on every view switch and read
    // landing must cost no layout reads unless boxes really stand.
    const readComing = deps.readComing();
    if (thread === null || missing <= 0 || !readComing || !deps.enabled()) {
      clear(box);
      return;
    }
    const plan = planPendingReplies(missing, readComing, true, replyHeights());
    if (plan) draw(box, plan);
    else clear(box);
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

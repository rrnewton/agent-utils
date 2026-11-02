// The links between Main's roots and their cards, and the glide to a link's other end (`#261
// two-column-connectors`, `#277 thread-card-focus`, `#280 scroll-stability`, `#298 thread-card-header`;
// `#624 voice-page-modularity`, moved from the legacy script into view/channel). The one owner of what
// #column-links last drew, the frame asked for to draw it again, the selection the links follow, and the
// wait before a glide. Which columns show, the selection and the highlighted card, and where the mouse
// is, are the page's.
//
// `#261 two-column-connectors`. The owner, 2026-10-12: "In two column view we get a list of threads on
// the right. The main channel view on the left contains the root message of ALL those threads, but
// there is no easy way to see how they match up. Some git diff style tools show connections between
// hunks on the left and the right with connector arrows (the kind with only right angle bends that
// leave the box on the left, travel through the middle vertical gap, and connect to the right side)."
//
// WHAT IS DRAWN, in one SVG over the main screen (#column-links), while two columns show the cards:
//
//   * For each root on Main whose thread has a card shown on the right, with both ends on screen, a
//     path in the thread's colour — the colour of its Thread(N) tag and of the card's left edge — out
//     of the root's right edge level with that tag, across to a lane in the gap between the columns,
//     up or down it, and into the card's left edge at its middle: a dot where it leaves, an arrowhead
//     where it arrives. Pairs with an end off screen are not drawn. Each link has a lane of its own,
//     in the order that crosses the fewest others (`laneOrder`).
//   * The SELECTED message or card, in either column (`#239 single-selection`), draws its link even
//     when the other end is off screen: from the end on screen to the gap, then up or down to the edge
//     of the other column, where an arrowhead points the way to it. Drawn over the others, heavier.
//   * A HIGHLIGHTED card (`#277 thread-card-focus`) takes the selection's place while it shows: its
//     link is the one drawn so, whatever the mouse is over, and every other link is grey.
//   * And if the mouse RESTS on the selected message or card — on the highlighted card, while one is
//     highlighted — for COLUMN_GLIDE_MS with the other end off screen, the other column scrolls
//     smoothly until that end is level with it (`glideToCounterpart`) — unless the pointer leaves it,
//     the selection or the highlight moves or the reader scrolls that column first. Where the reader's
//     system asks for reduced motion (the stylesheet's `--column-glide`), it jumps instead.
//   * ONLY A DWELL GLIDES (`#280 scroll-stability`). The owner: "it jumped me to the bottom of the
//     screen in the main message view (two column mode)... I don't think I touched anything". The
//     selection outlives the pointer: a mouse crossing the cards on its way to the dock or the top bar
//     selects each card it passes over and leaves the last one selected, and when the glide waited on
//     the selection alone, Main slid 3000px 1.5 s after the pointer had gone. So the wait is armed by a
//     real mouse move over what the glide follows (`pointRow` -> `dwellColumnGlide`, `glideFollows`),
//     or by a click highlighting a card under the mouse (`toggleLitCard`), dropped when the mouse moves
//     on from it or leaves the window (`driftFromColumnGlide`, `onPointerLeavePage`), armed again if it
//     comes back, and runs only if the mouse is still there (`pointerStays`). A selection drawn whole
//     that the list then moves out from under a pointer standing still has not been left: the pointer
//     is resting where it chose it. While a card is highlighted, the mouse over any other card or over
//     Main arms nothing. A selection or a highlight made any other way — ] or [ handing the keys to a
//     column, j or k, Tab, Enter on a card, a redraw handing it on — never glides: moving the focus
//     scrolls no list but the one the focus is in, and that only to show it.
//   * OVER A THREAD ON THE RIGHT (`#298 thread-card-header`), its card is at the top of the column, and
//     its one link is drawn, from its first message on Main to that card, always as the selection's:
//     clipped at Main's edge, pointing the way, while the root is above or below what Main shows; to
//     the line of the collapsed run the root is in; up from Main's top while the root is older than
//     every row Main has drawn; and not at all where the root is drawn nowhere else — hidden among
//     Main's rows by a filter — nor for a thread nobody has answered, which has no colour to draw it
//     in. The mouse resting on the card glides Main to the root, by the rules above, once each time
//     the thread is opened.
//
// WHAT IT COSTS (`#226 page-energy-profile`). Nothing in one column, on a phone, or while the page is
// hidden. Otherwise one pass per animation frame at most, asked for by a scroll of either column
// (passive listeners), a resize, a redraw of either list or a new selection: every rectangle it needs
// is read before anything is written, and only what changed is written, so a frame in which nothing
// moved writes nothing. Over a thread on the right that pass measures one link, its root's and its
// card's. Nothing is measured outside that pass but the one read a glide makes when its wait is up.

import { threadOf, timeOf } from "../../core/channels/index.ts";
import { drawLinkPath, laneOrder, linkAt, linkedRootThread, pointerOn, rowLinkY } from "./links.ts";
import { rowMessages, rowShown, shownRowFor } from "./rows.ts";

/** How long a selection stays with its other end off screen before that column scrolls to it, in ms. */
const COLUMN_GLIDE_MS = 1500;

/** The most room between two lanes in the gap, in px. */
const LINK_LANE_PX = 7;

/** How long an arrowhead is, in px; it is as wide again, a little more. */
const LINK_HEAD_PX = 7;

/** The radius of the dot where a link leaves its root, in px. */
const LINK_DOT_PX = 3;

/** How far inside the other column a glide brings the end it goes to, at least, in px. */
const LINK_GLIDE_MARGIN_PX = 24;

/**
 * How far the mouse may be from where it last moved on the selection a glide waits on, across or down,
 * and still be resting there, in px: a hand on a mouse, or the selection drawn whole and moving out from
 * under a pointer standing still. `#280 scroll-stability`.
 */
const GLIDE_REST_PX = 8;

/** A column the keys move through, as far as the links read it (the page's `KeyPane`). */
export interface LinkPane {
  readonly name: string;
  /** The row carrying `data-selected`, the one lit row of the page while this is the pane in use. */
  readonly lit: HTMLElement | null;
}

/**
 * A root on Main and the card its link goes to. `row` is null for a root older than every row Main has
 * drawn, whose link points up from Main's top (`#298 thread-card-header`).
 */
export interface LinkPair {
  readonly thread: string;
  readonly row: HTMLElement | null;
  readonly card: HTMLElement;
}

interface Point {
  readonly x: number;
  readonly y: number;
}

interface Span {
  readonly top: number;
  readonly bottom: number;
}

/** Where each pair's ends are, what each column shows, the gap, and the SVG's corner (`measureColumnLinks`). */
export interface MeasuredLinks {
  readonly corner: { readonly left: number; readonly top: number };
  readonly left: Span;
  readonly right: Span;
  readonly gap: { readonly from: number; readonly to: number };
  readonly ends: readonly {
    readonly pair: LinkPair;
    readonly from: Point;
    readonly to: Point;
    readonly fromSeen: boolean;
    readonly toSeen: boolean;
  }[];
}

/** One link as drawn: its thread, colour, whether it is the selection's or greyed, its clip, line and marks. */
export interface PlannedLink {
  readonly thread: string;
  readonly hue: number;
  readonly selected: boolean;
  readonly dimmed: boolean;
  readonly clip: string;
  readonly line: string;
  readonly marks: string;
}

/**
 * What the links are, from what `measureColumnLinks` read, as the paths to draw: for each, its thread,
 * its colour (`hueOf`), whether it is the selection's, which way it is clipped ("up" or "down", or
 * "both" ends shown), the line and the marks at its ends. The selection's is last, so it is drawn over
 * the rest.
 *
 * `lit`, the thread whose card is highlighted while it shows (`#277 thread-card-focus`): every other
 * link is `dimmed`, drawn grey, so the highlighted card's stands out.
 */
function planColumnLinks(measured: MeasuredLinks, chosen: LinkPair | null, lit: string | null,
  hueOf: (thread: string) => number): PlannedLink[] {
  const { corner, left, right, gap, ends } = measured;
  type Link = { end: MeasuredLinks["ends"][number]; selected: boolean; from: Point; to: Point; clip: string; edge: number };
  const links: Link[] = [];
  for (const end of ends) {
    const selected = chosen !== null && end.pair === chosen;
    if (end.fromSeen && end.toSeen) {
      links.push({ end, selected, from: end.from, to: end.to, clip: "both", edge: 0 });
    } else if (selected && (end.fromSeen || end.toSeen)) {
      // To the edge of the column the other end is out of, on the side it is out of: in the gap, which
      // is both columns' edge, no lower than either column's foot.
      const span = end.fromSeen ? right : left;
      const far = end.fromSeen ? end.to : end.from;
      const clip = far.y < span.top ? "up" : "down";
      const edge = clip === "up" ? span.top + 2 : Math.min(left.bottom, right.bottom) - 2;
      const from = end.fromSeen ? end.from : { x: end.from.x, y: edge };
      const to = end.fromSeen ? { x: end.to.x, y: edge } : end.to;
      links.push({ end, selected, from, to, clip, edge });
    }
  }
  if (links.length === 0) return [];
  // The lanes: in the gap between Main's list (or the furthest root, if one reaches past it) and the
  // cards, less an arrowhead's room before the cards.
  const reach = Math.max(gap.from, ...links.map((link) => link.end.from.x));
  let low = reach + 3;
  let high = gap.to - LINK_HEAD_PX - 3;
  if (high - low < 4) {
    const middle = (low + high) / 2;
    low = middle - 2;
    high = middle + 2;
  }
  const order = laneOrder(links);
  const step = order.length > 1 ? Math.min(LINK_LANE_PX, (high - low) / (order.length - 1)) : 0;
  const middle = (low + high) / 2;
  const lane = new Map(order.map((link, at) => [link, middle + (at - (order.length - 1) / 2) * step]));
  const x = (value: number): number => linkAt(value, corner.left);
  const y = (value: number): number => linkAt(value, corner.top);
  const head = LINK_HEAD_PX;
  const half = LINK_HEAD_PX * 0.6;
  const dot = (cx: number, cy: number): string =>
    `M ${cx - LINK_DOT_PX} ${cy} a ${LINK_DOT_PX} ${LINK_DOT_PX} 0 1 0 ${2 * LINK_DOT_PX} 0 a ${LINK_DOT_PX} ${LINK_DOT_PX} 0 1 0 ${-2 * LINK_DOT_PX} 0 Z`;
  const into = (tx: number, ty: number): string =>
    `M ${tx} ${ty} L ${tx - head} ${ty - half} L ${tx - head} ${ty + half} Z`;
  const pointing = (lx: number, ey: number, way: string): string => {
    const back = way === "up" ? head : -head;
    return `M ${lx} ${ey} L ${lx - half} ${ey + back} L ${lx + half} ${ey + back} Z`;
  };
  const plan = links.map((link) => {
    const lx = x(lane.get(link) as number);
    const fx = x(link.from.x);
    const fy = y(link.from.y);
    const tx = x(link.to.x);
    const ty = y(link.to.y);
    const back = link.clip === "up" ? head : -head;
    let line;
    let marks;
    if (link.clip === "both") {
      line = `M ${fx} ${fy} H ${lx} V ${ty} H ${tx - head}`;
      marks = `${dot(fx, fy)} ${into(tx, ty)}`;
    } else if (link.end.fromSeen) {
      line = `M ${fx} ${fy} H ${lx} V ${ty + back}`;
      marks = `${dot(fx, fy)} ${pointing(lx, ty, link.clip)}`;
    } else {
      line = `M ${lx} ${fy + back} V ${ty} H ${tx - head}`;
      marks = `${pointing(lx, fy, link.clip)} ${into(tx, ty)}`;
    }
    return { thread: link.end.pair.thread, hue: hueOf(link.end.pair.thread), selected: link.selected,
      dimmed: lit !== null && link.end.pair.thread !== lit, clip: link.clip, line, marks };
  });
  return [...plan.filter((link) => !link.selected), ...plan.filter((link) => link.selected)];
}

/** What the links ask of the page. */
export interface ColumnLinksDependencies {
  /** The element with this id in the left screen (`leftEl`). */
  lookup(id: string): HTMLElement;
  /** Two columns, the main screen, and the channel on it (`columnsShown`, `currentScreen`, `currentView`). */
  onTwoColumns(): boolean;
  /** What the right column shows: "list" for the cards, "thread" for a thread (`columns.side.mode`). */
  sideMode(): string;
  /** Main's list of rows, or null (`channelPane.list()`). */
  mainList(): HTMLElement | null;
  /** The thread the right column shows, and its root (`sideThreadChoice`), or null. */
  openThread(): { readonly id: string; readonly root?: unknown } | null;
  /** The pane in use (`activePane`), or null. */
  activePane(): LinkPane | null;
  /** The right column's pane (`sidePane`). */
  sidePane(): LinkPane;
  /** The thread whose card is highlighted while it shows on the right (`litCardOnRight`), or null. */
  litCard(): string | null;
  /** The card of thread `thread` on the right (`cardRowOnRight`), or null. */
  cardRow(thread: string): HTMLElement | null;
  /** The thread's colour (`threadHue`). */
  hue(thread: string): number;
  /** Where the mouse last moved (`InputKinds.mouseAt`, ./input.ts), or null. */
  mouse(): Point | null;
  /** Where `row` is drawn from, its head and the date rule over it included (`drawnTop`). */
  drawnTop(row: HTMLElement): number;
  /** The room left above a row brought into view (`REPLY_LANDING_GAP_PX`). */
  landingGap(): number;
}

/** The links' commands. */
export interface ColumnLinks {
  /** Whether the links have a place. */
  shown(): boolean;
  /** Ask for the links to be drawn again at the next animation frame. */
  schedule(): void;
  /** Draw the links as the page stands now. */
  draw(): void;
  /** The selection was drawn again: the links follow it. */
  noteSelection(): void;
  /** A real mouse move over `row` of `pane`, or a click that has just highlighted the card `row` under the mouse. */
  dwell(pane: LinkPane, row: HTMLElement): void;
  /** A real mouse move: one that has left what a glide waits on drops the wait. */
  drift(): void;
  /** The mouse leaving the window. */
  pointerLeft(event: { readonly pointerType?: string } | null | undefined): void;
  /** How a glide moves: "smooth", or "auto" where the stylesheet says so. */
  glideBehavior(): ScrollBehavior;
}

/** Nothing drawn, nothing asked, no wait. */
export function createColumnLinks(deps: ColumnLinksDependencies): ColumnLinks {
  /**
   * Whether the links have a place: two columns, the channel on the main screen, and on the right the
   * cards, or a thread with its card at the top (`#298 thread-card-header`).
   */
  function columnLinksShown(): boolean {
    return deps.onTwoColumns() && (deps.sideMode() === "list" || threadCardLinked());
  }

  /** Whether the right column shows a thread under its card, which the thread's one link goes to. `#298 thread-card-header`. */
  const threadCardLinked = (): boolean => deps.sideMode() === "thread" && !deps.lookup("side-thread-card").hidden;

  /**
   * Every root drawn on Main whose thread has a card shown on the right, with that card, in Main's order.
   * Read from the lists alone: nothing is laid out. Over a thread on the right, its one pair: its root
   * and the card at the top of the column (`threadCardPair`).
   */
  function columnLinkPairs(): LinkPair[] {
    if (deps.sideMode() === "thread") return threadCardPair();
    const cards = new Map<string, HTMLElement>();
    for (const card of [...deps.lookup("side-cards").children] as HTMLElement[]) {
      if (card.hasAttribute("data-card") && rowShown(card)) cards.set(String(card.getAttribute("data-id")), card);
    }
    const list = deps.mainList();
    const pairs: LinkPair[] = [];
    if (!list || cards.size === 0) return pairs;
    for (const row of [...list.children] as HTMLElement[]) {
      const thread = linkedRootThread(row);
      const card = thread === null ? undefined : cards.get(thread);
      if (thread !== null && card) pairs.push({ thread, row, card });
    }
    return pairs;
  }

  /**
   * The link over a thread on the right, `#298 thread-card-header`: from its root on Main to the card at
   * the top of the column, as a list of none or one. None for a thread nobody has answered, drawn in no
   * colour, nor where Main draws its root nowhere: hidden among its rows by a filter or a read mode.
   */
  function threadCardPair(): LinkPair[] {
    const card = deps.lookup("side-thread-card");
    const thread = card.getAttribute("data-thread");
    if (!thread || card.hidden || card.getAttribute("data-thread-state") !== "open") return [];
    const row = rootRowOnMain(thread);
    if (row) return [{ thread, row, card }];
    return rootBeforeMain(thread) ? [{ thread, row: null, card }] : [];
  }

  /**
   * The row on Main standing for thread `thread`'s root: the root's own row, or the line of the collapsed
   * run it is in (`#221 read-modes`). Null where it is not drawn, or drawn but hidden.
   */
  function rootRowOnMain(thread: string): HTMLElement | null {
    const list = deps.mainList();
    if (!list) return null;
    for (const row of [...list.children] as HTMLElement[]) {
      const first = rowMessages(row)[0];
      if (!first || !first.thread || first.thread.is_root !== true || threadOf(first) !== thread) continue;
      const shown = shownRowFor(row);
      return shown && rowShown(shown) ? shown : null;
    }
    return null;
  }

  /**
   * Whether thread `thread`'s root is older than every row Main has drawn, so that it is above them all,
   * not loaded: by the time it was posted against that of Main's first row. False where either is not
   * known.
   */
  function rootBeforeMain(thread: string): boolean {
    const list = deps.mainList();
    const first = list ? ([...list.children] as HTMLElement[]).map((row) => rowMessages(row)[0]).find(Boolean) : null;
    const open = deps.openThread();
    const root = open && open.id === thread ? open.root : null;
    const rootAt = root ? timeOf(root, "timestamp") : NaN;
    const firstAt = first ? timeOf(first, "timestamp") : NaN;
    return Number.isFinite(rootAt) && Number.isFinite(firstAt) && rootAt < firstAt;
  }

  /** The pair whose end is the selection drawn on the page — a root on Main, or a card — or null. */
  function selectedLinkPair(pairs: readonly LinkPair[]): LinkPair | null {
    const pane = deps.activePane();
    const lit = pane ? pane.lit : null;
    if (!lit) return null;
    return pairs.find((pair) => (pane === deps.sidePane() ? pair.card : pair.row) === lit) || null;
  }

  /**
   * The pair whose link is drawn heavy and over the rest: the highlighted card's while it shows, whatever
   * is selected (`#277 thread-card-focus`), else the selection's. Null when that end has no pair. Over a
   * thread on the right, its one pair, whatever is selected (`#298 thread-card-header`).
   */
  function chosenLinkPair(pairs: readonly LinkPair[]): LinkPair | null {
    if (deps.sideMode() === "thread") return pairs[0] || null;
    const lit = deps.litCard();
    return lit !== null ? pairs.find((pair) => pair.thread === lit) || null : selectedLinkPair(pairs);
  }

  /**
   * Where each pair's ends are now, read in one go and nothing written: `from` on the root's right edge
   * and `to` on the card's left edge, each with whether it shows in its column; what each column shows,
   * top to bottom; the gap between Main's list and the cards; and the SVG's own corner, which every
   * point it draws is measured from.
   *
   * Both columns are taken to start where the right one's list does, under its heading: the gap the
   * lanes run in is the two columns' edge, and a link drawn above that would cross the heading. Main's
   * list starts higher, under the pill and the search glass floating over its head, so a root up there
   * is under them too.
   *
   * `#298 thread-card-header`. Over a thread on the right the card is IN the heading, so there the right
   * column starts at the card's top, which keeps the card on screen, and Main at its own list's. A root
   * older than every row Main has drawn has no row: its end is taken to be above Main's top.
   */
  function measureColumnLinks(pairs: readonly LinkPair[]): MeasuredLinks {
    const corner = deps.lookup("column-links").getBoundingClientRect();
    const leftArea = deps.lookup("scroll-area");
    const rightArea = deps.lookup("side-scroll");
    const leftTop = leftArea.getBoundingClientRect().top;
    const rightTop = rightArea.getBoundingClientRect().top;
    const header = deps.sideMode() === "thread" ? deps.lookup("side-thread-card").getBoundingClientRect() : null;
    const top = Math.max(leftTop, rightTop);
    const left = { top: header ? leftTop : top, bottom: leftTop + (leftArea.clientHeight || 0) };
    const right = { top: header ? header.top : top, bottom: rightTop + (rightArea.clientHeight || 0) };
    const list = deps.mainList();
    // Over a thread the gap ends at the card or at the thread's rows under it, whichever is further
    // left: the line runs down beside both, and a lane inside the rows is drawn over them (`#385
    // thread-line-overlap`). Rows with no box yet stand nowhere.
    const rows = header ? deps.lookup("side-log").getBoundingClientRect() : null;
    const gap = {
      from: list ? list.getBoundingClientRect().right : 0,
      to: header ? (rows && rows.width > 0 ? Math.min(header.left, rows.left) : header.left)
        : deps.lookup("side-cards").getBoundingClientRect().left,
    };
    const ends = pairs.map((pair) => {
      const row = pair.row ? pair.row.getBoundingClientRect() : null;
      const card = pair.card.getBoundingClientRect();
      const from = pair.row && row ? { x: row.right, y: rowLinkY(pair.row, row) } : { x: gap.from, y: left.top - LINK_HEAD_PX };
      const to = { x: card.left, y: (card.top + card.bottom) / 2 };
      return {
        pair,
        from,
        to,
        fromSeen: from.y >= left.top && from.y <= left.bottom,
        toSeen: to.y >= right.top && to.y <= right.bottom,
      };
    });
    return { corner, left, right, gap, ends };
  }

  /** What #column-links last drew, so a frame that would draw the same writes nothing. */
  let columnLinksDrawn = "[]";

  /**
   * Draw `plan` in #column-links: two paths a link, its line and the marks at its ends, reused in place,
   * each attribute written only where it changed.
   */
  function renderColumnLinks(plan: readonly PlannedLink[]): void {
    const signature = JSON.stringify(plan);
    if (signature === columnLinksDrawn) return;
    columnLinksDrawn = signature;
    const svg = deps.lookup("column-links");
    const want = plan.length * 2;
    while (svg.children.length > want) svg.removeChild(svg.children[svg.children.length - 1] as Element);
    // In the SVG's own namespace, which the markup gave it: this file names no address, not even that one.
    while (svg.children.length < want) svg.append(document.createElementNS(svg.namespaceURI, "path"));
    plan.forEach((link, at) => {
      drawLinkPath(svg.children[2 * at] as SVGElement, "column-link", link, link.line);
      drawLinkPath(svg.children[2 * at + 1] as SVGElement, "column-link-marks", link, link.marks);
    });
  }

  /** Whether a frame has been asked for to draw the links in. */
  let columnLinksDue = false;

  /**
   * Ask for the links to be drawn again at the next animation frame: one frame however often it is asked
   * before then. Where they have no place, they go at once, and nothing is asked; while the page is
   * hidden, nothing at all, and coming back asks.
   */
  function scheduleColumnLinks(): void {
    if (!columnLinksShown()) {
      renderColumnLinks([]);
      return;
    }
    if (document.hidden || columnLinksDue || typeof requestAnimationFrame !== "function") return;
    columnLinksDue = true;
    requestAnimationFrame(() => {
      columnLinksDue = false;
      drawColumnLinks();
    });
  }

  /**
   * Draw the links as the page stands now: in the frame `scheduleColumnLinks` asked for, and in each
   * frame of the card at the top of a thread moving up (`flyThreadCard`, `#298 thread-card-header`).
   */
  function drawColumnLinks(): void {
    if (document.hidden) return;
    if (!columnLinksShown()) {
      renderColumnLinks([]);
      return;
    }
    const pairs = columnLinkPairs();
    renderColumnLinks(pairs.length
      ? planColumnLinks(measureColumnLinks(pairs), chosenLinkPair(pairs), deps.litCard(), deps.hue) : []);
  }

  /** The selection the links were last told of, as "pane:id", or "". */
  let linkedSelection = "";

  /**
   * The wait before a glide, while one is armed: the selection it is for, the column and where that column
   * was, where the mouse last moved on the selection, and the timer.
   */
  let columnGlide: { key: string; other: HTMLElement; at: number; rest: Point | null; timer: ReturnType<typeof setTimeout> } | null = null;

  /**
   * The selection, as "pane:id", whose glide has had its turn — its wait ran out with the mouse still on
   * it, whatever it then found — so that the mouse resting on it, or moving about inside it, does not arm
   * it again: once a selection, as before. "" when the selection moves.
   */
  let columnGlideSpent = "";

  /**
   * The selection was drawn again (`drawSelection`): the links follow it, and a selection that is new
   * drops the wait for the old one, whose other end is no longer the one to bring on screen.
   *
   * `#277 thread-card-focus`. While a card is highlighted, it is what the links follow instead, and what
   * the glide follows (`glideFollows`): its root is the end brought on screen, and the mouse passing over
   * other cards neither moves the links nor arms a wait.
   *
   * It arms none of its own. `#280 scroll-stability`: only the mouse resting on what the glide follows
   * does (`dwellColumnGlide`, from `pointRow`, and from `toggleLitCard` for a click highlighting the card
   * under the mouse). A selection or a highlight made any other way — the keys handed to a column by ] or
   * [, j or k, Tab, Enter on a card, a redraw handing it on because its message went under Hide read or its
   * card under Unread only — was not followed by the reader's eye to the other column, and that column
   * moving 1.5 s later was "something updating underneath".
   */
  function noteLinkedSelection(): void {
    const header = linkedThreadCard();
    const card = deps.litCard();
    const pane = deps.activePane();
    const lit = pane ? pane.lit : null;
    const key = header !== null ? `header:${header}` : card !== null ? `card:${card}`
      : pane && lit ? `${pane.name}:${lit.hasAttribute("data-card") ? "card:" : ""}${lit.getAttribute("data-id")}` : "";
    if (key !== linkedSelection) {
      linkedSelection = key;
      columnGlideSpent = "";
      dropColumnGlide();
    }
    scheduleColumnLinks();
  }

  /**
   * The thread whose card stands at the top of the right column beside Main, which its link and the glide
   * follow there whatever is selected (`#298 thread-card-header`); else null.
   */
  function linkedThreadCard(): string | null {
    if (!columnLinksShown() || deps.sideMode() !== "thread") return null;
    return deps.lookup("side-thread-card").getAttribute("data-thread");
  }

  /** Drop the wait before a glide, if one is armed. */
  function dropColumnGlide(): void {
    if (columnGlide === null) return;
    clearTimeout(columnGlide.timer);
    columnGlide = null;
  }

  /**
   * What the glide follows, and its column: the card at the top of the right column's thread beside Main
   * (`#298 thread-card-header`); else the highlighted card while it shows on the right beside Main (`#277
   * thread-card-focus`), whatever is selected; else the selection drawn. Null with none of them.
   */
  function glideFollows(): { pane: LinkPane; row: HTMLElement } | null {
    // `#298 thread-card-header`. Over a thread on the right, the card at its top.
    if (linkedThreadCard() !== null) return { pane: deps.sidePane(), row: deps.lookup("side-thread-card") };
    const card = deps.litCard();
    if (card !== null) {
      const row = deps.cardRow(card);
      return row ? { pane: deps.sidePane(), row } : null;
    }
    const pane = deps.activePane();
    return pane && pane.lit ? { pane, row: pane.lit } : null;
  }

  /**
   * A real mouse move over `row` of `pane` (`pointRow`), the selection having followed it there, or a
   * click that has just highlighted the card `row` under the mouse (`toggleLitCard`): when `row` is what
   * the glide follows (`glideFollows`) — a root with a card, or a card; the highlighted card while one is
   * — and no wait is armed for it and its glide has not had its turn, wait COLUMN_GLIDE_MS, noting where
   * the other column is, which the reader scrolling it in the meantime changes, and where the mouse is.
   * The mouse arriving on it, or coming back to it after moving on, starts the wait; moving about inside
   * it only notes where it is. While a card is highlighted, the mouse on any other card, or on Main, arms
   * nothing.
   */
  function dwellColumnGlide(pane: LinkPane, row: HTMLElement): void {
    const mouseAt = deps.mouse();
    const here = mouseAt ? { x: mouseAt.x, y: mouseAt.y } : null;
    const followed = glideFollows();
    const on = followed !== null && followed.pane === pane && followed.row === row;
    if (columnGlide !== null) {
      if (columnGlide.key === linkedSelection && on) columnGlide.rest = here;
      return;
    }
    if (linkedSelection === "" || columnGlideSpent === linkedSelection) return;
    if (!on || !columnLinksShown()) return;
    const linked = pane === deps.sidePane() ? row.hasAttribute("data-card") || row === deps.lookup("side-thread-card")
      : linkedRootThread(row) !== null;
    if (!linked) return;
    const other = deps.lookup(pane === deps.sidePane() ? "scroll-area" : "side-scroll");
    columnGlide = { key: linkedSelection, other, at: other.scrollTop, rest: here,
      timer: setTimeout(glideToCounterpart, COLUMN_GLIDE_MS) };
  }

  /**
   * Whether the mouse stays on what a glide waits on (`glideFollows`): on it, as the browser says by
   * `:hover`, or within GLIDE_REST_PX of where it last moved on it — a hand resting on the mouse, or the
   * selection drawn whole (`#257 summary-truncation`), or a card highlighted and shown whole (`#277
   * thread-card-focus`), and the list moving it out from under a pointer standing still.
   */
  function pointerStays(glide: { rest: Point | null }): boolean {
    const followed = glideFollows();
    if (pointerOn(followed ? followed.row : null)) return true;
    const mouseAt = deps.mouse();
    return glide.rest !== null && mouseAt !== null &&
      Math.abs(mouseAt.x - glide.rest.x) <= GLIDE_REST_PX && Math.abs(mouseAt.y - glide.rest.y) <= GLIDE_REST_PX;
  }

  /**
   * A real mouse move (`onPointerMove`): one that has taken the pointer off the selection a glide waits
   * on, and away from where it rested there, drops the wait. `#280 scroll-stability`: the mouse crossing
   * the cards on its way to the dock or the top bar leaves the last one selected, and glides nothing.
   */
  function driftFromColumnGlide(): void {
    if (columnGlide !== null && !pointerStays(columnGlide)) dropColumnGlide();
  }

  /** The mouse leaving the window, which no move inside it says: the wait before a glide is dropped. */
  function onPointerLeavePage(event: { readonly pointerType?: string } | null | undefined): void {
    if (!event || event.pointerType === "mouse") dropColumnGlide();
  }

  /**
   * The wait is up: if the selection is the same — the highlighted card, while one is — and the mouse
   * still rests on it (`pointerStays`), its end on screen and the other end off it, and the reader has not
   * scrolled the other column since, scroll that column until the other end is level with the selection —
   * kept inside it — smoothly, or at once where the reader's system asks for less motion.
   */
  function glideToCounterpart(): void {
    const glide = columnGlide;
    columnGlide = null;
    if (!glide || glide.key !== linkedSelection || document.hidden || !columnLinksShown()) return;
    const pairs = columnLinkPairs();
    const chosen = chosenLinkPair(pairs);
    // A root Main has not drawn has no place to be brought to. `#298 thread-card-header`.
    if (!chosen || !chosen.row) return;
    // `#280 scroll-stability`. Only a dwell: a mouse that has moved on glides nothing, and coming back to
    // the selection waits again.
    if (!pointerStays(glide)) return;
    columnGlideSpent = glide.key;
    if (Math.abs(glide.other.scrollTop - glide.at) > 1) return;
    const measured = measureColumnLinks([chosen]);
    const end = measured.ends[0] as MeasuredLinks["ends"][number];
    const onMain = glide.other === deps.lookup("side-scroll");
    const [own, far, ownSeen, farSeen] = onMain
      ? [end.from, end.to, end.fromSeen, end.toSeen] : [end.to, end.from, end.toSeen, end.fromSeen];
    if (!ownSeen || farSeen) return;
    const span = onMain ? measured.right : measured.left;
    let level = Math.max(span.top + LINK_GLIDE_MARGIN_PX, Math.min(span.bottom - LINK_GLIDE_MARGIN_PX, own.y));
    // A root brought to Main is brought whole, its head and the date rule over it included, not only the
    // point its link leaves from: `#324 live-pill-covers-row` moved Main's top edge down to the floating
    // line's foot, and a card heading the right column put the root's head above it, cut off.
    if (!onMain) {
      const head = deps.drawnTop(chosen.row) + level - far.y;
      const lowest = span.bottom - LINK_GLIDE_MARGIN_PX;
      if (head < span.top + deps.landingGap()) level = Math.min(lowest, level + span.top + deps.landingGap() - head);
    }
    const top = glide.other.scrollTop + far.y - level;
    const behavior = glideBehavior();
    if (typeof glide.other.scrollTo === "function") glide.other.scrollTo({ top, behavior });
    else glide.other.scrollTop = top;
  }

  /**
   * How a glide moves: "smooth", or "auto" — at once — where the stylesheet says so, which it does for a
   * reader whose system asks for reduced motion (`--column-glide` on #screen-main). The stylesheet's to
   * say, as everything about the layout is: the script never asks a media query itself.
   */
  function glideBehavior(): ScrollBehavior {
    const style = typeof window.getComputedStyle === "function" ? window.getComputedStyle(deps.lookup("screen-main")) : null;
    return style && String(style.getPropertyValue("--column-glide")).trim() === "auto" ? "auto" : "smooth";
  }

  return {
    shown: columnLinksShown,
    schedule: scheduleColumnLinks,
    draw: drawColumnLinks,
    noteSelection: noteLinkedSelection,
    dwell: dwellColumnGlide,
    drift: driftFromColumnGlide,
    pointerLeft: onPointerLeavePage,
    glideBehavior,
  };
}

// What the page and the tests use of it.
export { COLUMN_GLIDE_MS, GLIDE_REST_PX, LINK_DOT_PX, LINK_GLIDE_MARGIN_PX, LINK_HEAD_PX, LINK_LANE_PX, planColumnLinks };

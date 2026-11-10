// The card at the top of an open thread, and its fold to a bar (`#298 thread-card-header`,
// `#388 thread-header-collapse`; `#624 voice-page-modularity`, moved from the legacy script):
// what the card was last drawn from, whether its thread is one nobody has answered, the drawing
// of it, and the reader's fold choice kept on the device. The fold's storage key and stored
// value format are the setting's own, handed in untouched, so a saved choice survives this move
// byte for byte. Words, order and timing are as they were in the script.

import { childByClass, nodeWithin } from "../dom/index.ts";
import type { ReplyTally } from "../../core/channels/index.ts";
import type { ThreadChoice } from "../../present/rows/index.ts";

/** The fold choice, as this owner uses it (core/settings `createKeptSwitch`). */
export interface ThreadCardFoldChoice {
  on(): boolean;
  keep(on: boolean): void;
  set(on: boolean): void;
  stored(): boolean;
}

/** A card's view of its thread, as `cardView` hands it on. */
export interface ThreadCardView {
  readonly text: string;
  readonly mode: string;
  readonly cut: unknown;
}

/** What the thread card head asks of the page. */
export interface ThreadCardHeadDependencies {
  /** The fold choice, made by core/settings' factory, handed in as a maker (that layer is not this package's to import). */
  makeFoldChoice(): ThreadCardFoldChoice;
  /** The fold setting's own key (`THREAD_CARD_FOLD_KEY`), as the page names it for the settings sync. */
  foldKey: string;
  leftById(id: string): HTMLElement;
  unansweredRoot(root: unknown, count: unknown, held: number, exact: unknown): boolean;
  threadReplyCount(id: string, root: unknown): unknown;
  exactReplyCount(id: string, root: unknown, tally: ReplyTally): unknown;
  cardView(choice: ThreadChoice): ThreadCardView;
  briefAge(at: number): string;
  sideCardReplies(choice: ThreadChoice, tally: ReplyTally): string;
  sideCardUnread(choice: ThreadChoice, tally: ReplyTally): string;
  cardIsSummarized(choice: ThreadChoice): boolean;
  watchPendingSummaries(choices: readonly ThreadChoice[], cardId: string): void;
  threadHue(id: string): unknown;
  markCardLayout(card: HTMLElement, mode: string, cut: unknown): void;
  fillCardText(words: HTMLElement, view: ThreadCardView, text: string): void;
  setTextIfChanged(node: HTMLElement, text: string): void;
  fullLocalAt(when: Date): string | null;
  cardCountParts(prefix: string, replies: string, unread: string): Node[];
  scheduleColumnLinks(): void;
  drawnOnPage(node: HTMLElement): boolean;
  atBottom(area: HTMLElement, slack: number): boolean;
  overscrollRoom(area: HTMLElement): number;
  fitOverscroll(area: HTMLElement): void;
  followFoot(area: HTMLElement): void;
  sideShowing(): boolean;
  sideThreadChoice(): ThreadChoice | null;
  replyTally(): ReplyTally;
}

/** The thread card head's acts and the state they own. */
export interface ThreadCardHead {
  /** By the id of each card at the top of a thread, what it was last drawn from. */
  readonly threadCardsDrawn: Map<string, string>;
  /** The fold setting's own key, as handed in. */
  readonly THREAD_CARD_FOLD_KEY: string;
  /** The cards at the top of a thread, one per column, and the list each heads. */
  readonly THREAD_CARD_LISTS: ReadonlyArray<readonly [string, string]>;
  /** Whether the cards at the top of a thread are folded to a bar, kept on the device. */
  readonly threadCardFoldChoice: ThreadCardFoldChoice;
  threadUnanswered(choice: ThreadChoice, tally: ReplyTally): boolean;
  paintThreadCard(card: HTMLElement, choice: ThreadChoice | null, tally: ReplyTally): void;
  renderThreadCardFold(): void;
  threadCardPressed(card: HTMLElement, event: Event): void;
  setThreadCardFolded(folded: boolean): void;
  paintSideThreadCard(tally?: ReplyTally): void;
}

/** Make the thread card head. */
export function createThreadCardHead(deps: ThreadCardHeadDependencies): ThreadCardHead {
  /**
   * By the id of each card at the top of a thread, what it was last drawn from, so a pass that changes
   * nothing writes nothing to it.
   */
  const threadCardsDrawn = new Map<string, string>();

  /**
   * Whether thread `choice` is one nobody has answered, by the rule its grey Thread tag is drawn by
   * (`unansweredRoot`): no colour is spent on it.
   */
  function threadUnanswered(choice: ThreadChoice, tally: ReplyTally): boolean {
    const root = choice.root;
    if (!root || !root.thread) return false;
    const entry = tally.threads.get(choice.id);
    return deps.unansweredRoot(root, deps.threadReplyCount(choice.id, root), entry ? entry.held : 0,
      deps.exactReplyCount(choice.id, root, tally));
  }

  /**
   * Draw `card`, the card at the top of a thread, for thread `choice`, from `tally`: what the thread is
   * about, how long since it moved, its replies and unread, and its colour — or grey, for a thread nobody
   * has answered. Its parts are the elements whose ids are the card's and `-text`, `-age` and `-count`.
   * Written only when what it would draw has changed.
   */
  function paintThreadCard(card: HTMLElement, choice: ThreadChoice | null, tally: ReplyTally): void {
    if (!choice) return;
    const view = deps.cardView(choice);
    const text = view.text;
    const age = deps.briefAge(Number.isFinite(choice.at) ? choice.at : choice.started);
    const replies = deps.sideCardReplies(choice, tally);
    const unread = deps.sideCardUnread(choice, tally);
    const unanswered = threadUnanswered(choice, tally);
    const summarized = deps.cardIsSummarized(choice);
    // `#528 thread-card-markdown-and-cap`. Waiting for its summary, it reads the Threads list again soon.
    deps.watchPendingSummaries([choice], card.id);
    const signature = JSON.stringify([choice.id, view.mode, text, summarized, age, replies, unread, unanswered]);
    if (threadCardsDrawn.get(card.id) === signature) return;
    threadCardsDrawn.set(card.id, signature);
    if (card.getAttribute("data-thread") !== choice.id) card.setAttribute("data-thread", choice.id);
    // `#428 summary-cards-full`. As a list card's text, this card's summary, when that is what it says, is drawn whole.
    if (summarized !== card.hasAttribute("data-summarised-card")) {
      if (summarized) card.setAttribute("data-summarised-card", "");
      else card.removeAttribute("data-summarised-card");
    }
    const state = unanswered ? "start" : "open";
    if (card.getAttribute("data-thread-state") !== state) card.setAttribute("data-thread-state", state);
    const hue = String(deps.threadHue(choice.id));
    if (unanswered) card.style.removeProperty("--thread-hue");
    else if (card.style.getPropertyValue("--thread-hue") !== hue) card.style.setProperty("--thread-hue", hue);
    const button = deps.leftById(`${card.id}-text`);
    let words = childByClass(button, "side-card-text");
    if (!words) {
      words = document.createElement("span");
      words.className = "side-card-text";
      button.append(words);
    }
    // `#528 thread-card-markdown-and-cap`: its root's formatted words, as a list card's.
    deps.markCardLayout(card, view.mode, view.cut);
    deps.fillCardText(words, view, text);
    if (button.getAttribute("title") !== text) button.setAttribute("title", text);
    deps.setTextIfChanged(deps.leftById(`${card.id}-age`), age);
    // `#479 message-time-full-date`. Hovering the open thread's age says the day it last moved, whole.
    const cardLastMoved = Number.isFinite(choice.at) ? choice.at : choice.started;
    const ageNode = deps.leftById(`${card.id}-age`);
    const ageTitle = Number.isFinite(cardLastMoved) ? deps.fullLocalAt(new Date(cardLastMoved)) : null;
    if (ageTitle && ageNode.getAttribute("title") !== ageTitle) ageNode.setAttribute("title", ageTitle);
    // A span, not the list card's button: the thread is open already. Never "Open thread".
    const count = deps.leftById(`${card.id}-count`);
    count.replaceChildren(...deps.cardCountParts("side-card", replies, unread));
    if (count.hidden !== (!replies && !unread)) count.hidden = !replies && !unread;
    // The link to the card goes, or comes, with the thread's first reply.
    deps.scheduleColumnLinks();
  }

  const THREAD_CARD_FOLD_KEY = deps.foldKey;

  /** The cards at the top of a thread, one per column, and the list each heads. */
  const THREAD_CARD_LISTS: ReadonlyArray<readonly [string, string]> = [["thread-card", "scroll-area"], ["side-thread-card", "side-scroll"]];

  /**
   * Whether the cards at the top of a thread are folded to a bar: the reader's choice, kept on the device and
   * read from it here, as the page loads. ABSENT MEANS OPEN; a browser refusing storage reads as open
   * (`DEVICE_SETTINGS.threadCardFolded.refused`). One owner in web/src/core/settings/switches.ts
   * (`#624 voice-page-modularity`).
   */
  const threadCardFoldChoice = deps.makeFoldChoice();

  /** Draw the cards at the top of a thread folded or open, as `threadCardFoldChoice` says: written only where it changed. */
  function renderThreadCardFold(): void {
    for (const [id] of THREAD_CARD_LISTS) {
      const card = deps.leftById(id);
      if (card.hasAttribute("data-folded") !== threadCardFoldChoice.on()) {
        if (threadCardFoldChoice.on()) card.setAttribute("data-folded", "");
        else card.removeAttribute("data-folded");
      }
      const button = deps.leftById(`${id}-text`);
      const expanded = threadCardFoldChoice.on() ? "false" : "true";
      if (button.getAttribute("aria-expanded") !== expanded) button.setAttribute("aria-expanded", expanded);
    }
  }

  /**
   * A click on `card`, the card at the top of a thread: folded or opened, unless it was on the X or a link
   * in it, which do their own acts, or it ended a selection of the card's text made with the mouse.
   */
  function threadCardPressed(card: HTMLElement, event: Event): void {
    for (let node = event.target as Node | null; node && node !== card; node = node.parentNode) {
      const tag = String((node as HTMLElement).tagName || "").toLowerCase();
      if (tag === "a" || (tag === "button" && node !== deps.leftById(`${card.id}-text`))) return;
    }
    const selection = typeof getSelection === "function" ? getSelection() : null;
    if (selection && !selection.isCollapsed && nodeWithin(card, selection.anchorNode)) return;
    setThreadCardFolded(!threadCardFoldChoice.on());
  }

  /**
   * Fold the cards at the top of a thread to a bar, or open them: kept, and drawn, with every list a card
   * on screen heads scrolled by as far as its top moved, so the rows the reader sees stay where they were.
   * A reader exactly on the newest line is kept on it (`followFoot`); one a little above it stays where
   * they are, not pulled down the slack. In the room past the right column's foot (`#338
   * column-overscroll`) the room is fitted to the column at once: a fold leaves the reader where they are
   * in it, and an opening too, except at the room's end, which keeps its own rule — the last card's top no
   * higher than the middle of the column, which a shorter column has lower — and so lets the rows there
   * down by at most half the card's growth, in the frame the card opens rather than one after.
   */
  function setThreadCardFolded(folded: boolean): void {
    threadCardFoldChoice.keep(folded);
    const held = THREAD_CARD_LISTS.filter(([card]) => deps.drawnOnPage(deps.leftById(card))).map(([, id]) => {
      const area = deps.leftById(id);
      return { area, top: area.getBoundingClientRect().top, scrolled: area.scrollTop,
        foot: deps.atBottom(area, 1) && deps.overscrollRoom(area) === 0 };
    });
    threadCardFoldChoice.set(folded);
    renderThreadCardFold();
    // From the scroll position read before, not the one now: a list grown under a reader near its foot has
    // been clamped to its new end by the layout that measured it, and adding to that put them further up.
    for (const { area, top, scrolled, foot } of held) {
      area.scrollTop = scrolled + area.getBoundingClientRect().top - top;
      deps.fitOverscroll(area);
      if (foot) deps.followFoot(area);
    }
    // The line from the root meets the card, or the bar, at its middle.
    deps.scheduleColumnLinks();
  }

  /** The right column's card at the top of its thread, as the rows now stand, after a pass over either column's rows. */
  function paintSideThreadCard(tally: ReplyTally = deps.replyTally()): void {
    if (!deps.sideShowing()) return;
    paintThreadCard(deps.leftById("side-thread-card"), deps.sideThreadChoice(), tally);
  }

  return {
    threadCardsDrawn,
    THREAD_CARD_FOLD_KEY,
    THREAD_CARD_LISTS,
    threadCardFoldChoice,
    threadUnanswered,
    paintThreadCard,
    renderThreadCardFold,
    threadCardPressed,
    setThreadCardFolded,
    paintSideThreadCard,
  };
}

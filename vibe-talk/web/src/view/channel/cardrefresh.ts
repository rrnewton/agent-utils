// Thread card lines, from the page's side (`#624 voice-page-modularity`, moved from the legacy
// script into view/channel beside cardlines.ts): what a read says of the server's page build, a
// summary landing owed a re-read, and the collapsed-lines chord. Where a card stops, its head and
// continuation are createCardLines'; K and the collapsed count stay their settings owners',
// instantiated by the page and handed in.

import { collapsedLinesWords } from "../../present/rows/index.ts";

/** What the card refresh asks of the page. */
export interface CardRefreshDependencies {
  /** Take the server's page build; true when it changed (`serverFacts.notePageBuild`). */
  notePageBuild(build: string): boolean;
  /** Draw what a changed page build means for this page (the stale-page notice). */
  pageBuildChanged(): void;
  /** The view the main screen shows ("voice" or "discord"). */
  view(): string;
  /** The channel chosen now. */
  selectedChannelId(): unknown;
  /** Forget when this channel's Threads list was last read. */
  forgetThreadRead(channel: string): void;
  /** Read the Threads list again. */
  refreshThreadDirectory(): Promise<void>;
  /** Draw the thread heading again. */
  renderThreadHeading(): void;
  /** Draw the side column again. */
  renderSideColumn(): void;
  /** The list the column being worked on shows. */
  selectionView(): string;
  /** Draw a thread's reply links again. */
  renderReplyLinks(): void;
  /** Whether a node is a text field, where the chord is the field's. */
  isTextField(target: unknown): boolean;
  /** The collapsed-line count in force. */
  collapsedCurrent(): number;
  /** Put a collapsed-line count on the page; the count actually applied. */
  collapsedChanged(n: number): number;
  /** Say a thing on the status line. */
  setStatus(text: string): void;
}

/** The card refresh's commands. */
export interface CardRefresh {
  /** Take what read `payload` says of the server's page build, and draw what it means for this page. */
  noteServerPageBuild(payload: unknown): void;
  /** A summary on its way is owed a read: the Threads list read now, past its spacing, and the cards drawn again. */
  summaryWatchFired(): Promise<void>;
  /**
   * Ctrl+( and Ctrl+) (Cmd on a Mac): one collapsed line fewer or more, and the status line says
   * the count it came to. Never in a text field, where the chord is the field's.
   */
  collapsedLinesFromKeyboard(direction: 1 | -1, target: unknown): boolean;
}

/** Made where the page wires its card lines, in the page's order. */
export function createCardRefresh(deps: CardRefreshDependencies): CardRefresh {
  function noteServerPageBuild(payload: unknown): void {
    const build = payload && typeof (payload as { page_build?: unknown }).page_build === "string"
      ? (payload as { page_build: string }).page_build
      : "";
    if (!deps.notePageBuild(build)) return;
    deps.pageBuildChanged();
  }

  async function summaryWatchFired(): Promise<void> {
    if (deps.view() !== "discord") return;
    deps.forgetThreadRead(String(deps.selectedChannelId()));
    await deps.refreshThreadDirectory();
    deps.renderThreadHeading();
    deps.renderSideColumn();
    // A thread open across the window: its root row is the card's inward child or not by the card's layout.
    if (deps.selectionView() === "thread") deps.renderReplyLinks();
  }

  function collapsedLinesFromKeyboard(direction: 1 | -1, target: unknown): boolean {
    if (deps.isTextField(target)) return false;
    const n = deps.collapsedChanged(deps.collapsedCurrent() + direction);
    deps.setStatus(`Collapsed messages show ${collapsedLinesWords(n)}.`);
    return true;
  }

  return { noteServerPageBuild, summaryWatchFired, collapsedLinesFromKeyboard };
}

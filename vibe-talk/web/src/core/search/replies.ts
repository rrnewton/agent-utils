// The cards' walk back through All's older pages for the search (`#578 search-status-clarity`, `#606
// server-side-search`; `#624 voice-page-modularity`, moved from the legacy script into core/search): the
// walk under way and what the last one found, and the walk itself — reading All's older pages, its newest
// first where All has no cursor, until its pages, its time or its finds run out, the reader stops it, or
// something takes it over — and how it ended. Reading and folding a page, drawing the control and saying what
// was found are the page's; why the cards offer it at all is told where the legacy script opens the section.

import type { CancelSignal, Cancellation } from "../../ports/index.ts";

/** A cards' walk under way, as the page draws it and the walk keeps it. */
export interface RepliesWalkState {
  /** What the walk depends on (the channel and the search): a change in either ends it. */
  readonly key: string;
  /** The channel's store it was started over. */
  readonly canon: unknown;
  /** The channel it reads. */
  readonly channel: string;
  /** The cards found already, which nothing this walk reads is counted as having found. */
  readonly held: Set<string>;
  /** The threads the pages it read brought a message of: only their cards are its finds. */
  readonly brought: Set<string>;
  /** The threads of the replies it read that say every word of the search. */
  readonly matched: Set<string>;
  /** Where All goes on back from: its own cursor while it is live, else the one its newest page gives. */
  cursor: string | null;
  cursorRead: boolean;
  pages: number;
  read: number;
  found: number;
  arrived: number;
  readonly started: number;
  readonly controller: Cancellation | null;
  stopping: boolean;
  ended: boolean;
  promise: Promise<void> | null;
}

/** A page of All, as far as the walk reads it. */
export interface RepliesPage {
  readonly messages?: readonly unknown[];
  readonly has_more?: unknown;
  readonly next_before?: string | null;
}

/** What the walk asks of the page. */
export interface RepliesWalkDependencies {
  /** Now, in milliseconds. */
  now(): number;
  /** At most this many older pages a walk reads (`SEARCH_OLDER_PAGES`). */
  readonly pages: number;
  /** Within this long, in milliseconds (`SEARCH_OLDER_MS`). */
  readonly budgetMs: number;
  /** Ending at the end of the page that brings this many threads (`SEARCH_OLDER_HITS`). */
  readonly hits: number;
  /** A press this soon after the walk began is not a Stop (`SEARCH_OLDER_TAP_MS`). */
  readonly tapMs: number;
  /** Whether the cards offer the walk now. */
  offered(): boolean;
  /** End the columns' walks and a filter change's walk back: one walk at a time. The columns' still ending. */
  endOthers(): Promise<unknown>[];
  /** All's own cursor, while All is live and says more exists; else null. */
  cursor(): string | null;
  /** What the walk depends on now (the channel and the search). */
  key(): string;
  /** The channel's store now. */
  canon(): unknown;
  /** The channel chosen now. */
  channel(): string;
  /** The cards the search and the read mode show now, by thread id. */
  cardsFound(): string[];
  /** A cancellation for the walk's reads (an `AbortController`), where the browser has them. */
  controller(): Cancellation | null;
  /** Whether a message read says every word of the search, as it stands as the walk begins. */
  matcher(): (message: unknown) => boolean;
  /** A message's thread id, or null. */
  threadOf(message: unknown): string | null;
  /** Whether a message is its thread's first. */
  isRoot(message: unknown): boolean;
  /** Whether All reaches the start of the channel. */
  allLoaded(): boolean;
  /** Whether the walk is still the reader's, as the page sees it: the same search and store, the cards still up. */
  holds(walk: RepliesWalkState): boolean;
  /** The store's copies of what it holds, for a read of All's newest page to fold over. */
  heldNow(): unknown;
  /** Read a page of All in `channel`: its newest where `before` is null, else the one before it. */
  read(channel: string, before: string | null, signal: CancelSignal | undefined): Promise<RepliesPage>;
  /** Fold a page the walk read into the channel's store (`foldRepliesPage`). */
  fold(page: RepliesPage, newest: boolean, held: unknown, walk: RepliesWalkState): void;
  /** Draw the control and its note again (`renderRepliesSearch`). */
  render(): void;
  /** The walk failed: say on the status line that the search stopped. */
  failed(): void;
  /** What an ended walk says under its control (`repliesWalkNote`). */
  ending(walk: RepliesWalkState, reason: string, failure: unknown): string;
}

/** The walk's commands and questions. */
export interface RepliesWalk {
  /** The walk under way, or null. */
  current(): RepliesWalkState | null;
  /** What the last walk found, for the search `key()` names; forgotten once the search changes. */
  noteFor(key: () => string): string;
  /** Forget what the last walk found. */
  forgetNote(): void;
  /** Whether walk `walk` is still the reader's: not ended, the one under way, and still holding for the page. */
  holds(walk: RepliesWalkState): boolean;
  /** End the walk where it is, saying nothing of it; its read on the wire is aborted. Its promise, or null. */
  end(): Promise<void> | null;
  /** Walk back for the search, where the cards offer it and none is under way. */
  search(): Promise<void>;
  /** Stop the walk at the end of the read on the wire, which is aborted. */
  stop(): void;
  /** The control pressed: Stop while a walk is under way (not in its first moments), else a walk. */
  pressed(): Promise<void> | undefined;
}

/** No walk under way. */
export function createRepliesWalk(deps: RepliesWalkDependencies): RepliesWalk {
  /** The cards' walk back through All's older pages for the search, while one is under way, or null. */
  let repliesWalk: RepliesWalkState | null = null;
  /** What the last cards' walk found, under its control, until the search changes, or null. */
  let repliesNote: { key: string; text: string } | null = null;

  function holds(walk: RepliesWalkState): boolean {
    return !walk.ended && repliesWalk === walk && deps.holds(walk);
  }

  function stop(): void {
    const walk = repliesWalk;
    if (!walk || walk.stopping) return;
    walk.stopping = true;
    if (walk.controller) walk.controller.abort();
    deps.render();
  }

  function search(): Promise<void> {
    if (!deps.offered() || repliesWalk !== null) return Promise.resolve();
    // One at a time on the page: a column's walk ends, and is waited for, so two reads never overlap. A
    // filter change's walk back gives way.
    const others = deps.endOthers();
    const cursor = deps.cursor();
    const walk: RepliesWalkState = {
      key: deps.key(),
      canon: deps.canon(),
      channel: deps.channel(),
      held: new Set(deps.cardsFound()),
      brought: new Set(),
      matched: new Set(),
      cursor,
      cursorRead: false,
      pages: 0,
      read: 0,
      found: 0,
      arrived: 0,
      started: deps.now(),
      controller: deps.controller(),
      stopping: false,
      ended: false,
      promise: null,
    };
    repliesWalk = walk;
    repliesNote = null;
    deps.render();
    const count = (): void => {
      walk.found = deps.cardsFound().filter((id) => !walk.held.has(id) && walk.brought.has(id)).length;
    };
    const matches = deps.matcher();
    const note = (page: RepliesPage): void => {
      for (const message of page.messages || []) {
        const id = deps.threadOf(message);
        if (!id) continue;
        walk.brought.add(id);
        // `#606 server-side-search`: a reply the shared language matches, on its own.
        if (!deps.isRoot(message) && matches(message)) walk.matched.add(id);
      }
    };
    walk.promise = (async () => {
      let reason = "done";
      /** What the read that failed threw. */
      let failure: unknown = null;
      try {
        if (others.length > 0) await Promise.all(others.map((one) => one.catch(() => {})));
        let last = 0;
        while (walk.pages < deps.pages) {
          if (!holds(walk) || walk.stopping) break;
          if (deps.allLoaded()) {
            reason = "start";
            break;
          }
          if (walk.pages > 0 && deps.now() - walk.started + last > deps.budgetMs) {
            reason = "budget";
            break;
          }
          const owed = walk.cursor === null;
          if (owed && walk.cursorRead) {
            reason = "nowhere";
            break;
          }
          walk.cursorRead = walk.cursorRead || owed;
          if (!owed) walk.pages += 1;
          deps.render();
          const held = owed ? deps.heldNow() : null;
          const began = deps.now();
          const page = await deps.read(walk.channel, owed ? null : String(walk.cursor),
            walk.controller ? walk.controller.signal : undefined);
          if (!owed) last = deps.now() - began;
          if (!holds(walk)) break;
          deps.fold(page, owed, held, walk);
          note(page);
          walk.cursor = page.has_more === true ? page.next_before || null : null;
          if (!owed) {
            walk.read += 1;
            walk.arrived += (page.messages || []).length;
          }
          count();
          // How many have come so far, on the control (`searchWalkProgress`).
          deps.render();
          if (walk.stopping || walk.found >= deps.hits) break;
        }
        if (walk.stopping) reason = "stopped";
        else if (reason === "done" && deps.allLoaded()) reason = "start";
      } catch (error) {
        if (walk.ended || walk.stopping || (walk.controller && walk.controller.signal.aborted)) {
          reason = walk.stopping ? "stopped" : "done";
        } else {
          reason = "failed";
          failure = error;
          // The failure itself is reported where every failed read is; this says the search stopped, and
          // the note under the control says why.
          deps.failed();
          throw error;
        }
      } finally {
        if (repliesWalk === walk) {
          // A walk another search, another list or another channel has taken over from says nothing there.
          const stands = holds(walk);
          if (stands) count();
          repliesWalk = null;
          const text = stands ? deps.ending(walk, reason, failure) : "";
          repliesNote = text ? { key: walk.key, text } : null;
          deps.render();
        }
      }
    })();
    return walk.promise;
  }

  return {
    current: () => repliesWalk,
    noteFor(key) {
      if (repliesNote && repliesNote.key !== key()) repliesNote = null;
      return repliesNote ? repliesNote.text : "";
    },
    forgetNote() {
      repliesNote = null;
    },
    holds,
    end() {
      const walk = repliesWalk;
      if (!walk) return null;
      walk.ended = true;
      repliesWalk = null;
      if (walk.controller) walk.controller.abort();
      deps.render();
      return walk.promise;
    },
    search,
    stop,
    pressed() {
      const walk = repliesWalk;
      if (walk) {
        if (deps.now() - walk.started >= deps.tapMs) stop();
        return undefined;
      }
      return search();
    },
  };
}

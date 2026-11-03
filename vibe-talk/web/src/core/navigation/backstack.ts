// The page's own history entries, kept in step with what is open (`#304`, `#423 back-key-consistency`;
// `#624 voice-page-modularity`, moved from the legacy script into core/navigation): what the entries above
// the page's base stand for, a going-back the page asked for and has not seen land, an entry put off until
// the reader's first touch, the levels a Back could not close, how deep the earlier load's entries reach,
// and where the browser was when the page last looked. Pushing, replacing and going back through entries
// as levels open and close, closing levels for a Back the browser made, and adopting an earlier load's
// entries at start. What is open (the levels) and how each closes are the page's, as are the window's
// listeners; THE HISTORY MIRRORS WHAT IS OPEN and the rest are told where the legacy script opens the
// section.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/** The key the page's own history entries keep their facts under, in `history.state`. */
const BACK_STATE_KEY = "vibeTalkBack";
/** How long a going-back the page asked for may take to land before the page stops waiting for it. */
const BACK_TRAVERSAL_MS = 2000;
/** The deepest an earlier load's entry is taken to be: more than anything the page opens is ever deep. */
const BACK_DEPTH_MAX = 16;

/** The window's history, as far as the entries use it. */
export interface BackHistoryPort {
  readonly state: unknown;
  pushState(state: unknown, title: string): void;
  replaceState(state: unknown, title: string): void;
  back(): void;
  go(delta: number): void;
  scrollRestoration?: string;
}

/** An entry's facts: the page load that made it, how deep it is, and what it stands for. */
export interface BackEntryFacts {
  readonly session: string;
  readonly depth: number;
  readonly tag: unknown;
}

/** What the entries ask of the page. */
export interface BackStackDependencies {
  readonly clock: ClockPort;
  /** This page load's own name for the entries it makes: an entry with another is from before a reload. */
  readonly session: string;
  /** The window's history, where it can be written; null in a host without one. */
  history(): BackHistoryPort | null;
  /** Where the browser is in the tab's history, by the Navigation API where it has one; null elsewhere. */
  index(): number | null;
  /** Whether the page may push an entry now: not before the reader has ever touched it, where the browser says. */
  mayPush(): boolean;
  /** What a Back would close now, the first-opened first (`backLevels`). */
  levels(): string[];
  /** Close level `tag` as its own Back, X or Escape closes it (`closeBackLevel`). */
  close(tag: string): void;
  /** A close threw: say `message` (`showError`). */
  failed(message: string): void;
  /** Note the place the reader is at (`notePlace`), at the start of a sync. */
  notePlace(): void;
  /** Put the address in step with the place (`syncDeepLinkAddress`). */
  syncAddress(): void;
  /** Apply the link the page opened by, once there is a channel to apply it to (`applyDeepLink`, guarded). */
  applyLink(): void;
}

/** The entries' commands and questions. */
export interface BackStack {
  /** Whether the page keeps its history in step yet: from the end of the script, once every part is defined. */
  started(): boolean;
  /** Whether an entry is wanted but put off until the reader's first touch. */
  deferred(): boolean;
  /**
   * Keep the history in step from here on, taking the entry the page opened on for its own: at the depth an
   * earlier load gave it, never more than the browser says there are entries of the page's own under it; or,
   * holding anything else, as its base. True where there is a history to listen to.
   */
  start(): boolean;
  /** Put the history in step with what is open, at the end of the task that changed it. */
  schedule(): void;
  /** The browser went back (or forward) to the entry holding `state`: close one thing for each entry gone back through. */
  pop(state: unknown): void;
}

/** Not started. */
export function createBackStack(deps: BackStackDependencies): BackStack {
  const BACK_SESSION = deps.session;
  /** Whether the page keeps its history in step yet: from the end of the script, once every part is defined. */
  let backStarted = false;
  /** What the page's entries above its base stand for, the first-opened first: as many as the entry it is on is deep. */
  let backTags: string[] = [];
  /** A going-back the page asked for and has not yet seen land: the depth it goes to, and the timer that stops waiting. */
  let backTraversal: { target: number; timer: TimerHandle } | null = null;
  /** A going-back never landed: none is asked for again until the browser's history moves. */
  let backTraversalStuck = false;
  /** The levels, as `backLevels` says them, that a Back could not close: nothing is pushed for them again until they change. */
  let backRefused: string | null = null;
  /** An entry is wanted, but the reader has not touched the page yet: it is pushed at their first touch. */
  let backDeferred = false;
  /** Whether a `syncBackStack` is already due at the end of this task. */
  let backSyncDue = false;
  /** Where the browser was in the tab's history when the page last looked (`historyIndex`), to tell a Back from a Forward. */
  let backIndex: number | null = null;
  /**
   * How deep the earlier load's entries under this load's reach: one an earlier load made, less deep than
   * this, is taken for this load's own, at the depth it says. Nought where the page opened on none.
   */
  let backAdopted = 0;

  /** The state of the page's entry at `depth`, standing for `tag`. */
  function backEntry(depth: number, tag: unknown): Record<string, BackEntryFacts> {
    return { [BACK_STATE_KEY]: { session: BACK_SESSION, depth, tag } };
  }

  /** An entry's facts, where it holds any: no more than an object. */
  function factsOf(state: unknown): { session?: unknown; depth?: unknown; tag?: unknown } | null {
    const facts = state && typeof state === "object" ? (state as Record<string, unknown>)[BACK_STATE_KEY] : null;
    return facts && typeof facts === "object" ? facts as { session?: unknown; depth?: unknown; tag?: unknown } : null;
  }

  /** The facts of an entry this page load made, or null: no state, somebody else's, or one from before a reload. */
  function liveBackEntry(state: unknown): BackEntryFacts | null {
    const facts = factsOf(state);
    if (!facts || facts.session !== BACK_SESSION) return null;
    return Number.isInteger(facts.depth) && (facts.depth as number) >= 0 ? facts as BackEntryFacts : null;
  }

  /** The facts of an entry an earlier load of the page made — another load's name — or null. */
  function earlierLoadEntry(state: unknown): BackEntryFacts | null {
    const facts = factsOf(state);
    if (!facts || typeof facts.session !== "string" || facts.session === BACK_SESSION) return null;
    return Number.isInteger(facts.depth) && (facts.depth as number) >= 0 ? facts as BackEntryFacts : null;
  }

  /**
   * The facts of an entry the page stands on: one this load made, or one of the earlier load's under it
   * (`backAdopted`), which the page takes for its own at the depth it says. Null for any other.
   */
  function standingBackEntry(state: unknown): BackEntryFacts | null {
    const live = liveBackEntry(state);
    if (live) return live;
    const earlier = earlierLoadEntry(state);
    return earlier && earlier.depth < backAdopted ? earlier : null;
  }

  /** `backTags` cut or padded to `depth`: what the page knows of the entries up to the one it is on. */
  function fitBackTags(depth: number): string[] {
    const tags = backTags.slice(0, depth);
    while (tags.length < depth) tags.push("");
    return tags;
  }

  /** Make the entry the browser is on this load's own, at `depth`, standing for `tag` ("top" at depth nought). */
  function ownBackEntry(history: BackHistoryPort, depth: number, tag: string): void {
    const named = depth === 0 ? "top" : tag;
    history.replaceState(backEntry(depth, named), "");
    backTags = fitBackTags(depth);
    if (depth > 0) backTags[depth - 1] = named;
  }

  /**
   * Close from the top until no more than `target` levels are open. A level its close leaves open is
   * noted (`backRefused`) and the closing stops: pushing an entry for it again would make a Back that
   * never gets out.
   */
  function closeBackLevelsTo(target: number): void {
    for (let step = 0; step < 16; step += 1) {
      const levels = deps.levels();
      if (levels.length <= target) return;
      const said = levels.join(" ");
      try {
        deps.close(levels[levels.length - 1] as string);
      } catch (error) {
        deps.failed(error && (error as Error).message ? (error as Error).message : String(error));
      }
      if (deps.levels().join(" ") === said) {
        backRefused = said;
        return;
      }
      // `#543 in-app-message-links`. Undoing a jump puts back everything under it as it was, whatever
      // is open as it begins: nothing under it is closed on its account.
      if (levels[levels.length - 1] === "jump") return;
    }
  }

  /** Put the history in step with what is open, at the end of the task that changed it. */
  function scheduleBackSync(): void {
    if (!backStarted || backSyncDue) return;
    backSyncDue = true;
    void Promise.resolve().then(() => {
      // `#423 back-key-consistency`. Whatever moved the history, the place the reader is at now.
      try {
        deps.notePlace();
      } catch (_error) {
        // A place that cannot be read is not noted: Back and Forward go on through the others.
      }
      try {
        syncBackStack();
      } catch (_error) {
        // A browser that refuses a history call leaves the page as it was, with no entry to go back to.
        backSyncDue = false;
      }
      // `#462 webapp-deep-links`. The address in step with the place just noted; and the place a
      // link named, applied once there is a channel to apply it to (a no-op from then on).
      try {
        deps.syncAddress();
      } catch (_error) {
        // An address that cannot be written leaves the last one standing.
      }
      deps.applyLink();
    });
  }

  /**
   * Put the history in step with the levels: an entry pushed for each level opened, the top one replaced
   * for a level that gave way to another, and entries gone back through for levels closed. The depth is
   * read from the browser's entry each time, never only from what the page remembers, so a push the
   * browser ignored is not counted, and nothing is ever gone back through below the page's own entry: the
   * page never takes the reader out of the app.
   */
  function syncBackStack(): void {
    backSyncDue = false;
    if (!backStarted) return;
    const history = deps.history();
    if (!history || backTraversal) return;
    const levels = deps.levels();
    const said = levels.join(" ");
    if (backRefused !== null && backRefused !== said) backRefused = null;
    const entry = liveBackEntry(history.state);
    if (!entry) {
      history.replaceState(backEntry(0, "top"), "");
      backTags = [];
    } else if (entry.depth !== backTags.length) {
      backTags = fitBackTags(entry.depth);
    }
    const depth = backTags.length;
    const wanted = backRefused === null ? levels.length : Math.min(levels.length, depth);
    if (wanted < depth) {
      if (!backTraversalStuck) goBackThrough(history, depth - wanted);
      return;
    }
    backDeferred = wanted > depth && !deps.mayPush();
    if (wanted > depth && !backDeferred) {
      for (let at = depth; at < wanted; at += 1) {
        history.pushState(backEntry(at + 1, levels[at]), "");
        const now = liveBackEntry(history.state);
        // A push the browser ignored (it throttles a flood of them) is not counted.
        if (!now || now.depth !== at + 1) break;
        backTags.push(levels[at] as string);
      }
      backTraversalStuck = false;
    }
    const top = backTags.length;
    if (top > 0 && levels[top - 1] !== undefined && backTags[top - 1] !== levels[top - 1]) {
      history.replaceState(backEntry(top, levels[top - 1]), "");
      backTags[top - 1] = levels[top - 1] as string;
    }
    backIndex = deps.index();
  }

  /** Go back through the page's top `count` entries, for levels already closed, and wait for it to land. */
  function goBackThrough(history: BackHistoryPort, count: number): void {
    const target = backTags.length - count;
    backTags = backTags.slice(0, target);
    backTraversal = { target, timer: deps.clock.setTimeout(backTraversalLost, BACK_TRAVERSAL_MS) };
    if (count === 1) history.back();
    else history.go(-count);
  }

  /** A going-back that never landed: the page believes the browser's entry, and asks for no more until it moves. */
  function backTraversalLost(): void {
    if (!backTraversal) return;
    backTraversal = null;
    backTraversalStuck = true;
    const history = deps.history();
    const entry = history ? liveBackEntry(history.state) : null;
    backTags = fitBackTags(entry ? entry.depth : 0);
    scheduleBackSync();
  }

  return {
    started: () => backStarted,
    deferred: () => backDeferred,
    start() {
      backStarted = true;
      const history = deps.history();
      if (!history) return false;
      try {
        history.scrollRestoration = "manual";
      } catch (_error) {
        // A host whose history will not take it moves nothing on a Back anyway: the page does not scroll.
      }
      const earlier = earlierLoadEntry(history.state);
      const index = deps.index();
      const depth = earlier ? Math.min(earlier.depth, index === null ? BACK_DEPTH_MAX : index, BACK_DEPTH_MAX) : 0;
      ownBackEntry(history, depth, earlier ? String(earlier.tag) : "");
      backAdopted = depth;
      backIndex = index;
      return true;
    },
    schedule: scheduleBackSync,
    pop(state) {
      if (!backStarted) return;
      const history = deps.history();
      if (!history) return;
      const awaited = backTraversal;
      if (awaited) {
        deps.clock.clearTimeout(awaited.timer);
        backTraversal = null;
      }
      backTraversalStuck = false;
      const from = backTags.length;
      const index = deps.index();
      const moved = index !== null && backIndex !== null ? index - backIndex : null;
      backIndex = index;
      const entry = standingBackEntry(state);
      const wasOpen = deps.levels().length;
      let target;
      let closeTo;
      if (entry) {
        target = entry.depth;
        // As many things as entries gone back through, from the top: down to the entry's depth where the
        // history is in step, and no further where something open still waits for its entry (`backDeferred`).
        closeTo = Math.max(target, wasOpen - Math.max(0, from - target));
      } else if (awaited) {
        // The page's own going-back, landed somewhere it cannot read: where it asked to go.
        target = awaited.target;
        closeTo = target;
      } else if (moved !== null && moved > 0) {
        // A Forward, as the Navigation API says, onto an entry nobody can say the depth of: above where the
        // page was by as many as it moved, so that nothing closes and the page goes back down to what is open.
        target = from + moved;
        closeTo = wasOpen;
      } else {
        // A Back onto one: the top thing open closes, and nothing more.
        target = Math.max(0, wasOpen - 1);
        closeTo = target;
      }
      if (!entry || entry.session !== BACK_SESSION) {
        ownBackEntry(history, target, entry ? String(entry.tag) : "");
        // Under an entry of the earlier load's, the rest of that load's; under one nobody can account for, nothing.
        if (entry) backAdopted = target;
        else if (!awaited && !(moved !== null && moved > 0)) backAdopted = 0;
      } else {
        backTags = fitBackTags(target);
      }
      if (!awaited || target !== awaited.target) closeBackLevelsTo(closeTo);
      syncBackStack();
      // `#462 webapp-deep-links`. The entry landed on carries the address it was left with, which
      // may name a place the reader has since left (entries are pushed as levels open, places move
      // after): say where the reader stands now, on this entry. A further traversal the sync just
      // started lands with its own pop, and that one says it then.
      try {
        if (!backTraversal) deps.syncAddress();
      } catch (_error) {
        // An address that cannot be written leaves the entry's own standing.
      }
    },
  };
}

// What the page and the tests use of it.
export { BACK_DEPTH_MAX, BACK_STATE_KEY, BACK_TRAVERSAL_MS };

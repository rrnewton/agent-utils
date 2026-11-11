// Which column the code is working on (`#229 desktop-two-column`; `#624 voice-page-modularity`, moved from the
// legacy script into view/channel): the left one, except inside `inColumn(columns.side, ...)`, which makes the
// right column the one worked on, runs, and puts the left column back before it returns, so nothing is left
// pointing at the right column between one event and the next. Each column says whether it has been worked
// on and left since it was last made afresh (`left`). The columns and their records are the page's.

import type { Message, ThreadSummary } from "../../contract/index.ts";
import { elementIn } from "../dom/index.ts";
import type { ElementById } from "../dom/index.ts";

/** A column, as far as working on it reads it. */
export interface WorkedColumn {
  /** Whether it has been worked on and left since it was last made afresh. */
  left: boolean;
}

/** The working column's commands and questions. */
export interface ColumnWork<C extends WorkedColumn> {
  /** The column being worked on now. */
  active(): C;
  /** Whether that is the right column (for `el`, which asks on every call). */
  side(): boolean;
  /**
   * Work on column `column` from now on: its records are the ones the page reaches. The column put aside has
   * been worked on and `left`; the one taken up has not.
   */
  useColumn(column: C): void;
  /**
   * Run `fn` working on column `column`, and work on the one before again however it returns. What `fn`
   * returns is returned: a promise from an `async` function, whose work after its first wait is no longer in here.
   */
  inColumn<T>(column: C, fn: () => T): T;
}

/**
 * The ColumnWork port (`#624 voice-page-modularity`, step 17 move 3, published for the lanes that
 * consume the columns): the working column's whole surface, as `createColumnWork` makes it. Owners
 * that switch columns (the page, the column's own views) take the port; readers that must never
 * move the working column take only `ColumnWorkReader` below. No other package reaches the
 * captured `activeColumn`/`sideActive` behind it.
 */
export type ColumnWorkPort<C extends WorkedColumn> = ColumnWork<C>;

/**
 * The working column read, never switched: which column is worked on, and whether it is the right
 * one. This is the shape consumers such as the older-pages walks already take inline; naming it
 * here makes the port the one place that shape is defined.
 */
export type ColumnWorkReader<C extends WorkedColumn> = Pick<ColumnWork<C>, "active" | "side">;

/** Working on the left column. */
export function createColumnWork<C extends WorkedColumn>(columns: { readonly main: C; readonly side: C }): ColumnWork<C> {
  /** The column the code is working on now. The left one, except inside `inColumn`. */
  let activeColumn: C = columns.main;
  /** Whether that is the right column. */
  let sideActive = false;

  function useColumn(column: C): void {
    if (column === activeColumn) return;
    activeColumn.left = true;
    column.left = false;
    activeColumn = column;
    sideActive = column === columns.side;
  }

  return {
    active: () => activeColumn,
    side: () => sideActive,
    useColumn,
    inColumn(column, fn) {
      const was = activeColumn;
      if (column === was) return fn();
      useColumn(column);
      try {
        return fn();
      } finally {
        useColumn(was);
      }
    },
  };
}

/** The elements of the channel's pane, and the right column's that stand in for each. */
export const COLUMN_IDS = {
  "scroll-area": "side-scroll",
  "pane-discord": "side-pane",
  "channel-summary": "side-summary",
  "summary-note": "side-summary-note",
  "inbox-note": "side-inbox-note",
  "clear-backlog": "side-clear-backlog",
  "load-older": "side-older",
  "search-older-note": "side-search-older-note",
  "timeline-notice": "side-notice",
  "channel-loading": "side-loading",
  "thread-list": "side-thread-list",
  "discord-log": "side-log",
  "pinned-log": "side-pinned-log",
  "outgoing-log": "side-outgoing-log",
  "channel-composer": "side-composer",
  "channel-compose-text": "side-compose-text",
  "channel-send": "side-send",
  // `#1135 raw-message-page`: the row under the box and Cancel's question, in each column.
  "channel-raw": "side-raw",
  "channel-cancel": "side-cancel",
  "channel-discard": "side-discard",
  "channel-discard-text": "side-discard-text",
  "channel-discard-yes": "side-discard-yes",
  "channel-discard-no": "side-discard-no",
  "channel-compose-state": "side-compose-state",
  "search-empty": "side-search-empty",
  "scroll-tools": "side-tools",
  "jump-newest": "side-jump-newest",
  "jump-newest-label": "side-jump-newest-label",
  // `#277 reply-in-column`. Where a reply opened in the column is written, at the column's foot.
  "column-reply": "side-reply",
};

/** What a search of one column counted, for the count in the bar. */
export interface ColumnSearchCount {
  matched: number;
  loaded: number;
  tally: unknown;
  pinned: boolean;
}

/** One column's own records, by field group (`#624 voice-page-modularity`, plan step 15). */
export interface PageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather> extends WorkedColumn {
  name: string;
  searched: ColumnSearchCount | null;
  searchWalk: unknown;
  searchNote: { key: string; text: string } | null;
  older: Older;
  arrows: Arrows;
  load: Load;
  entry: Entry;
  pins: Pins;
  tools: Tools;
  filter: Filter;
  list: List;
  rows: Rows;
  selection: Selection;
  composer: Composer;
  done: Done;
  place: Place;
  gather: Gather;
}

/** The right column: a page column, and what it shows. */
export interface SidePageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather>
  extends PageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather> {
  mode: "list" | "thread";
  fromCards: boolean;
}

/** A fresh record of each field group, as the page makes them. */
export interface ColumnFreshMakers<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather> {
  older(): Older;
  arrows(): Arrows;
  load(): Load;
  entry(): Entry;
  pins(): Pins;
  tools(): Tools;
  filter(): Filter;
  list(): List;
  rows(): Rows;
  selection(view?: string): Selection;
  composer(): Composer;
  done(): Done;
  place(): Place;
  gather(): Gather;
}

/**
 * The two columns, each with its own fresh records: Main on the left, and on the right the list of
 * threads or one thread. Nothing is shared between them; a record made for one is never the other's.
 */
export function createColumns<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather>(
  fresh: ColumnFreshMakers<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather>,
): {
  main: PageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather>;
  side: SidePageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather>;
} {
  const column = (name: string, selection: Selection): PageColumn<Older, Arrows, Load, Entry, Pins, Tools, Filter, List, Rows, Selection, Composer, Done, Place, Gather> => ({
    name, left: false, searched: null, searchWalk: null, searchNote: null,
    older: fresh.older(), arrows: fresh.arrows(), load: fresh.load(), entry: fresh.entry(),
    pins: fresh.pins(), tools: fresh.tools(), filter: fresh.filter(), list: fresh.list(),
    rows: fresh.rows(), selection, composer: fresh.composer(), done: fresh.done(),
    place: fresh.place(), gather: fresh.gather(),
  });
  return {
    main: column("main", fresh.selection("main")),
    side: { ...column("side", fresh.selection()), mode: "list", fromCards: false },
  };
}

/** A column's record of its reads' state (`freshLoadState`). */
export interface LoadState {
  freshness: string;
  freshAt: number;
  refreshing: boolean;
  lineTimer: ReturnType<typeof setTimeout> | null;
  generation: number;
}

/** A column's record of where its list stands against its newest message (`freshScrollPlace`). */
export interface ColumnListPlace {
  jumpWanted: boolean | number;
  inScrollback: boolean;
}

/** A column's record of its scroll tools' pass (`freshScrollTools`). */
export interface ScrollToolsState {
  held: number;
  owed: boolean;
}

/**
 * A column's own record of its reads' state (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `freshness` and `freshAt` and `refreshing` below; `lineTimer`, the loading line's
 * timer (`renderChannelLoading`); `generation`, bumped by each read the column starts, so a completed
 * read can tell whether it still describes the column (`currentDiscordLoad`).
 *
 */
export function freshLoadState(): LoadState {
  return { freshness: "fresh", freshAt: 0, refreshing: false, lineTimer: null, generation: 0 };
}

/**
 * A column's own record of where its list stands against its newest message (`#624
 * voice-page-modularity`, plan step 15): `jumpWanted`, the arrival its jump is for;
 * `inScrollback`, whether it is in its scrollback. The folds owner reads them
 * (web/src/view/channel/folding.ts).
 *
 * @returns {{jumpWanted: boolean | number, inScrollback: boolean}}
 */
export function freshScrollPlace(): ColumnListPlace {
  return { jumpWanted: false, inScrollback: false };
}

/**
 * A column's own record of its scroll tools' pass: `held`, how many redraws are under way that
 * pass over the scroll tools once, at their end; `owed`, whether a pass was asked for inside one.
 *
 * @returns {{held: number, owed: boolean}}
 */
export function freshScrollTools(): ScrollToolsState {
  return { held: 0, owed: false };
}

/**
 * The working column's own records under their page names: its reads' state (`loadState`), where
 * its list stands (`scrollPlace`) and its scroll tools' pass (`scrollTools`).
 */
export interface ColumnStateAccess {
  loadState(): LoadState;
  scrollPlace(): ColumnListPlace;
  scrollTools(): ScrollToolsState;
  selection(): SelectionRecord;
  timelineRows(): TimelineRowsRecord;
  composerState(): ComposerRecord;
  viewKey(view?: string, thread?: unknown): string;
}

/** The column state access over the working column (`columnWork`). */
export function createColumnStateAccess(
  columnWork: ColumnWorkReader<{
    load: LoadState; place: ColumnListPlace; tools: ScrollToolsState;
    selection: SelectionRecord; rows: TimelineRowsRecord; composer: ComposerRecord;
  } & WorkedColumn>,
): ColumnStateAccess {
  /** The column being worked on's record of where its list stands (`freshScrollPlace`). */
  function scrollPlace(): ColumnListPlace {
    return columnWork.active().place;
  }

  /** The column being worked on's record of its scroll tools' pass (`freshScrollTools`). */
  function scrollTools(): ScrollToolsState {
    return columnWork.active().tools;
  }

  /** The column being worked on's record of its reads' state (`freshLoadState`). */
  function loadState(): LoadState {
    return columnWork.active().load;
  }

  /** The column being worked on's record of what it is showing (`freshSelection`). */
  function selection(): SelectionRecord {
    return columnWork.active().selection;
  }

  /** The column being worked on's record of the rows its list was last drawn from (`freshTimelineRows`). */
  function timelineRows(): TimelineRowsRecord {
    return columnWork.active().rows;
  }

  /** The column being worked on's record of its message box (`freshComposerState`). */
  function composerState(): ComposerRecord {
    return columnWork.active().composer;
  }
  /** The key of a view of the column being worked on, the thread's for a thread (`viewKey`). */
  function viewKey(view: string = selection().view, thread: unknown = selection().threadId): string {
    return view === "thread" ? `thread:${thread}` : view;
  }

  return { loadState, scrollPlace, scrollTools, selection, timelineRows, composerState, viewKey };
}

/** A column's record of what it is showing (`freshSelection`). */
export interface SelectionRecord {
  view: string;
  threadId: string | null;
  thread: ThreadSummary | null;
  origin: string;
}

/** A column's record of the rows its list was last drawn from (`freshTimelineRows`). */
export interface TimelineRowsRecord {
  messages: Message[];
  threads: ThreadSummary[];
}

/** A column's record of its message box (`freshComposerState`). */
export interface ComposerRecord {
  key: string;
}

/**
 * A column's own record of what it is showing (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `view`, which list (main, threads, flat or thread); `threadId` and `thread`, the
 * thread selected and its summary; `origin`, the list a thread was opened from. The left column starts
 * on Main only until the channel's provider is known (`openChannelView` decides it from then on); the
 * right column starts on a thread.
 *
 */
export function freshSelection(view = "thread"): SelectionRecord {
  return { view, threadId: null, thread: null, origin: "main" };
}

/**
 * A column's own record of the rows its list was last drawn from (`#624 voice-page-modularity`, plan
 * step 15, as `freshOlderPages`): `messages`, the messages of a Main, All or thread list; `threads`,
 * the thread summaries of the Threads list.
 *
 */
export function freshTimelineRows(): TimelineRowsRecord {
  return { messages: [], threads: [] };
}

/**
 * A column's own record of its message box (`#624 voice-page-modularity`, plan step 15, as
 * `freshOlderPages`): `key`, the box the column is composing in (`channelComposerKey`), whose draft
 * `channelDrafts` holds.
 *
 */
/** A column's pinned-only view record: only pinned rows, and what was last drawn. */
export interface PinsViewRecord {
  only: boolean;
  drawn: string;
}

/** A fresh pinned-only view record (`#624 voice-page-modularity`). */
export function freshPinsView(): PinsViewRecord {
  return { only: false, drawn: "" };
}

export function freshComposerState(): ComposerRecord {
  return { key: "" };
}

/** Finding the page's elements in the column being worked on, and in the left one. */
export interface ElementLookup {
  /**
   * The element with this id: in the column being worked on, for the ids of the channel's pane (see
   * `COLUMN_IDS`). Typed by the kind of element voice.html gives the id (web/src/view/dom `ElementById`,
   * generated from the markup by web/ids.mjs, `#624 voice-page-modularity`), so the checker knows `.value`
   * and `.checked` exist on a form control; an id not known as a literal is a plain HTMLElement.
   */
  el<K extends string>(id: K): K extends keyof ElementById ? ElementById[K] : HTMLElement;
  /**
   * The element with this id in the LEFT column whichever column is being worked on: for the few
   * things that belong to the left column alone (the call's pane beside the channel's, the chips and
   * the pull gesture over #scroll-area, the keyboard's pane for it) and may be asked about from inside
   * `inColumn`.
   */
  leftEl(id: string): HTMLElement;
}

/** The element lookup over the working column (`columnWork`). */
export function createElementLookup(columnWork: ColumnWorkReader<WorkedColumn>): ElementLookup {
  function el<K extends string>(id: K): K extends keyof ElementById ? ElementById[K] : HTMLElement {
    // The column worked on decides (web/src/view/dom `elementIn`, `#624 voice-page-modularity`).
    // Typed by the id, as the JSDoc above says; the lookup itself answers an HTMLElement.
    return elementIn(id, columnWork.side(), COLUMN_IDS) as K extends keyof ElementById ? ElementById[K] : HTMLElement;
  }

  function leftEl(id: string): HTMLElement {
    return document.getElementById(id) as HTMLElement;
  }

  return { el, leftEl };
}

// The reply open in the stream, and what it was opened with (`#410 inline-reply-editor`, `#185
// reply-new-thread`, `#229 desktop-two-column`, `#267 quote-reply`, `#314 reply-focus-flake`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): one record the page reads and
// writes as the reply opens, moves, is put away and closes. Opening, docking and sending it, and every rule
// about where it goes, are the page's (`openReply`, `dockReply`, `closeReply`, `sendReply`).

/**
 * Where the reply on screen goes, as `replyRoute` decided when it opened — or since, when a read placed a
 * message the stream had left unplaced, or a reply arrived in the thread it offered to start
 * (`followReplyStore`).
 */
export interface ReplyRouting {
  thread: string | null;
  branch: string | null;
  into: string | null;
  unknown: boolean;
  removed: boolean;
  note: string;
}

/** The reply's record. */
export interface ReplyState<Column, Message> {
  /** The message the reply answers, or null while none is open. */
  target: Message | null;
  /** The channel it was opened in. */
  channelId: string | null;
  /** Where it goes (`ReplyRouting`). */
  routing: ReplyRouting;
  /** The column it was opened from, whose list its place is in. */
  column: Column;
  /**
   * Whether a reply is open: its box in the stream of the list Reply was pressed in (see `dockReply`).
   * There is no other place for it to be open.
   */
  docked: boolean;
  /**
   * A reply put away because the list it was open in went — two columns becoming one — to be opened again
   * in the stream, its words as they were, as soon as its message is drawn (`reopenPendingReply`).
   * `typing`: the box had the focus, and gets it back if that is at once. `ready`: the layout that followed
   * is drawn, so a list on screen is one to open it in. Null when none waits.
   */
  pending: { id: string; typing: boolean; ready: boolean } | null;
  /**
   * Whether the browser took the focus off the box of a reply open in a column because it stopped drawing
   * that column, so that the reader was typing in it as far as the page is concerned
   * (`noteReplyFocusLeaving`).
   */
  focusTaken: boolean;
  /**
   * What the box was opened with when it was opened with a quote, and the draft that was there before it,
   * so that Cancel on a quote left untouched keeps only that draft. Null otherwise.
   */
  quote: { opened: string; draft: string } | null;
  /** The list the reply open in the stream was opened on, in `column`; null while none is open. */
  list: HTMLElement | null;
  /** Whether opening it gathered its message's replies, which putting it down puts back. */
  gathered: boolean;
  /** The row the box is drawn under, while it is drawn under one (`placeInlineReply`). */
  anchor: HTMLElement | null;
}

/** No reply open, from column `column`. */
export function createReplyState<Column, Message>(column: Column): ReplyState<Column, Message> {
  return {
    target: null,
    channelId: null,
    routing: { thread: null, branch: null, into: null, unknown: false, removed: false, note: "" },
    column,
    docked: false,
    pending: null,
    focusTaken: false,
    quote: null,
    list: null,
    gathered: false,
    anchor: null,
  };
}

// The threads sends in the outbox go into, before they are there (`#466 reply-promotes-thread`; `#624
// voice-page-modularity`, moved from the legacy script into core/outbox): which thread of the channel on
// screen each send posts into, and, worked out once for each change of the outbox, the store's messages
// or the channel, how many sends count in each thread and when the newest was written. Drawing the
// counts, the cards and the rows, and placing a reply's target, are the page's; why a send counts in its
// thread before the server says so is told where the legacy script opens the section.

/** A send as far as its thread is read: where it continues, its thread, what it answers, and its state. */
export interface PendingSend {
  readonly continueIn?: unknown;
  readonly threadId?: unknown;
  readonly replyTo?: unknown;
  readonly channel: string;
  readonly state?: string;
  readonly remaining?: unknown;
  readonly echoed?: unknown;
  readonly createdAt?: unknown;
}

/** What the pending threads ask of the page. */
export interface PendingThreadsDependencies<Message> {
  /** The channel on screen (`selectedChannelId`). */
  channel(): string;
  /** How often the outbox has changed (`outbox.version`). */
  outboxVersion(): number;
  /** The sends in the outbox (`outbox.values`). */
  sends(): Iterable<PendingSend>;
  /** The store on screen as it stands, never loaded from here: its channel and messages. */
  store(): { readonly channel: string; readonly messages: readonly Message[] };
  /** Whether a reply in channel `channel` is a thread reply (`channelRepliesInThread`, Slack). */
  repliesInThread(channel: string): boolean;
  /** Message `message`'s own thread record (`heldThreadRecord`), or null. */
  threadRecordOf(message: Message): { readonly id: unknown } | null;
  /** Message `message`'s id. */
  idOf(message: Message): unknown;
}

/** The pending threads' questions. */
export interface PendingThreads {
  /**
   * The thread `entry` posts into in the channel on screen: where the rest of a cut post continues, the
   * thread it was written in, or — where a reply is a thread reply — the thread of the message it
   * answers, by that message's own thread record. Null for the main channel, or a message whose thread
   * is not known.
   */
  threadOf(entry: PendingSend): string | null;
  /** The sends in the outbox for the channel on screen that count in thread `thread`. */
  into(thread: unknown): number;
  /** The threads of the channel on screen sends go into, each with when its newest was written. */
  times(): Map<string, number>;
  /** Which threads of the channel on screen have sends counting in them, and how many: for redraws. */
  key(): string;
  /**
   * The rows are being passed over for the pending threads: whether `key()` changed since they last were
   * (a send going into a thread, or gone from the outbox, changes that thread's count and its card), noting it.
   */
  noteDrawn(): boolean;
}

/** Worked out at the first question. */
export function createPendingThreads<Message>(deps: PendingThreadsDependencies<Message>): PendingThreads {
  function outgoingThreadOf(entry: PendingSend): string | null {
    if (entry.continueIn) return String(entry.continueIn);
    if (entry.threadId) return String(entry.threadId);
    const viewing = String(deps.channel());
    if (!entry.replyTo || entry.channel !== viewing || deps.store().channel !== viewing ||
        !deps.repliesInThread(entry.channel)) return null;
    // The store as it stands, never loaded from here: this runs in the passes that draw it.
    const answered = deps.store().messages.find((held) => String(deps.idOf(held)) === String(entry.replyTo));
    const record = answered ? deps.threadRecordOf(answered) : null;
    return record ? String(record.id) : null;
  }

  /** `key()` as it stood when the rows were last passed over for it (`noteDrawn`). */
  let drawnKey = "";

  /** `pendingByThread`'s last answer, and what it was worked out for. */
  let pendingCache = { key: "", threads: new Map<string, { count: number; at: number }>() };

  /**
   * By thread of the channel on screen, the sends in the outbox counting in it — not yet sent, and not yet
   * seen on the stream (`noteOutgoingEcho`), whose copy the store then holds and counts — and when the
   * newest was written. Worked out once for each change of the outbox, the store's messages or the
   * channel, and not for every row of every pass that asks (`into`).
   */
  function pendingByThread(): Map<string, { count: number; at: number }> {
    const viewing = String(deps.channel());
    const key = `${viewing} ${deps.outboxVersion()} ${deps.store().channel} ${deps.store().messages.length}`;
    if (pendingCache.key === key) return pendingCache.threads;
    const threads = new Map<string, { count: number; at: number }>();
    for (const entry of deps.sends()) {
      if (entry.channel !== viewing || entry.state === "sent" || !entry.remaining || entry.echoed) continue;
      const thread = outgoingThreadOf(entry);
      if (thread === null) continue;
      const held = threads.get(thread) || { count: 0, at: 0 };
      threads.set(thread, { count: held.count + 1, at: Math.max(held.at, Number(entry.createdAt) || 0) });
    }
    pendingCache = { key, threads };
    return threads;
  }

  return {
    threadOf: outgoingThreadOf,
    into(thread) {
      const held = pendingByThread().get(String(thread));
      return held ? held.count : 0;
    },
    times: () => new Map([...pendingByThread()].map(([thread, { at }]) => [thread, at])),
    key: pendingKey,
    noteDrawn() {
      const key = pendingKey();
      if (key === drawnKey) return false;
      drawnKey = key;
      return true;
    },
  };

  function pendingKey(): string {
    const viewing = String(deps.channel());
    return [viewing, ...[...pendingByThread()].sort().map(([thread, { count }]) => `${thread}=${count}`)].join(" ");
  }
}

// What Hide read's own list said of itself, in a channel without threads (`#50 todo-view`, `#62
// message-count-accuracy`, `#196 auto-read-noise`, `#240 reading-bookmark`; `#624 voice-page-modularity`,
// moved from the legacy script into core/channels): how many messages are left, of how large a window,
// how many of it the server left out as read automatically, whether the window is the whole channel, when
// its oldest message was sent, and which channel it is. Kept rather than recomputed from the read, because
// the read is not the only thing that changes the list: a message arriving adds a row with no read at all,
// and a Done takes one away, and the head of the list must move with them. Saying it is the page's.

/** What the last `/todo` read said, as the list now stands. */
export interface TodoState {
  /** Messages still to deal with. */
  readonly left: number;
  /** The recent window they are a subset of. */
  readonly window: number;
  /** How many of the window the server left out as read automatically. */
  readonly noise: number;
  /** Whether the window is the whole channel. */
  readonly complete: boolean;
  /** When the window's oldest message was sent, left or not, or null. */
  readonly from: string | null;
  /** The channel's id: not the name it wore, which an alias can change without a read. */
  readonly channelId: string | null;
}

/** A `/todo` answer, as far as it speaks of itself. */
export interface TodoRead {
  readonly messages?: readonly unknown[];
  readonly window?: unknown;
  readonly noise?: unknown;
  readonly complete?: unknown;
  readonly window_from?: unknown;
  readonly channel?: { readonly id: string } | null;
}

/** The to-do list's commands and questions. */
export interface TodoView {
  /** What it says now. */
  state(): TodoState;
  /** Take down what a `/todo` answer said about itself. */
  note(payload: TodoRead): void;
  /** A message arrived on the list: one more to do, in a window one larger. */
  arrived(): void;
  /** The list now holds `left` messages to do (a Done took some away). */
  setLeft(left: number): void;
  /**
   * Whether the list, for channel `channel`, holds every message still to do from time `at` on: the window
   * less what is dealt with, so a message it leaves out from when the window starts is dealt with. Older
   * than the window, nothing is known.
   */
  holdsFrom(at: number, channel: string): boolean;
}

/** Nothing read yet. */
export function createTodoView(): TodoView {
  let todoView: TodoState = { left: 0, window: 0, noise: 0, complete: false, from: null, channelId: null };
  return {
    state: () => todoView,
    note(payload) {
      const left = (payload.messages || []).length;
      todoView = {
        left,
        window: typeof payload.window === "number" ? payload.window : left,
        // `#196 auto-read-noise`. How many of the window the server left out as read automatically.
        // The rows are not here to count, so Settings' count comes from this in to-do mode.
        noise: typeof payload.noise === "number" ? payload.noise : 0,
        complete: payload.complete === true,
        // When the window's oldest message was sent, left or not. `#240 reading-bookmark`: whether a
        // message the list leaves out is in the window, so read (`holdsFrom`).
        from: typeof payload.window_from === "string" ? payload.window_from : null,
        // The ID, not the name it was wearing at the time. `#39 channel-alias` lets the owner rename
        // a channel from Settings without re-reading it, and a name captured here would leave this
        // line saying what the channel used to be called.
        channelId: payload.channel ? payload.channel.id : null,
      };
    },
    arrived() {
      todoView = {
        ...todoView,
        left: todoView.left + 1,
        // The channel gained a message too, so the SUBSET and the window it is a subset of both grow.
        // Leaving the window alone would read as the backlog catching up with a channel that stood
        // still.
        window: todoView.window + 1,
      };
    },
    setLeft(left) {
      todoView = { ...todoView, left };
    },
    holdsFrom(at, channel) {
      if (String(todoView.channelId) !== String(channel)) return false;
      if (todoView.complete) return true;
      const from = Date.parse(String(todoView.from || ""));
      return Number.isFinite(from) && at >= from;
    },
  };
}

// Where the reader is (`#624 voice-page-modularity`, plan step 9): the channel chosen to read. The
// one owner of the selection; the bar's picker is a projection of it, drawn by whoever subscribes.
// No browser history and no pixel positions: those are the shell's and the columns'.

/** Where the reader is, as one read-only record. */
export interface NavigationSnapshot {
  /** The id of the channel chosen to read, or "" for none. */
  readonly channelId: string;
}

/** The selection's owner. */
export interface Navigation {
  /** Where the reader is now. */
  snapshot(): NavigationSnapshot;
  /**
   * Choose channel `channelId` ("" for none). Every subscriber is told, in the order they
   * subscribed, before this returns, whether or not it changed: a choice of the channel already
   * chosen still redraws whatever projects it.
   */
  selectChannel(channelId: string): void;
  /** Be told of every choice, until the returned function is called. */
  subscribe(listener: (snapshot: NavigationSnapshot) => void): () => void;
}

/** A selection with nothing chosen. */
export function createNavigation(): Navigation {
  let current: NavigationSnapshot = Object.freeze({ channelId: "" });
  const listeners = new Set<(snapshot: NavigationSnapshot) => void>();
  return {
    snapshot: () => current,
    selectChannel(channelId: string): void {
      current = Object.freeze({ channelId });
      for (const listener of [...listeners]) listener(current);
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

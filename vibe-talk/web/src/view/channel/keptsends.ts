// What the page kept of its sends, read back at load (`#51 reply-view`, `#500 sent-receipts-pile-up`, `#892
// voice-page-optimistic-done`, `#193 compose-draft-loss`, `#420 bookmark-clear`; `#624 voice-page-modularity`, moved
// from the legacy script's wiring into view/channel for the plan's step 17): the replies' drafts, this device's own
// posts and the outbox, the Done writes a page closed before the server had them (sent now, when the connection
// comes back, and handed over as the page goes), the sends dismissed in the last days, and the Undo and Forget
// buttons over them. What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface KeptSendsDependencies {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** The replies' drafts read back; this device's own posts, before the outbox, whose confirmed sends are no longer
   * saved; and the outbox. */
  loadReplyDrafts(): unknown;
  loadOwnPosts(): unknown;
  loadOutgoingMessages(): unknown;
  /** `#892 voice-page-optimistic-done`. The Done writes left over sent, and this page's handed over as it goes. */
  sendLeftOverInboxWrites(): unknown;
  leaveInbox(): unknown;
  /** `#193 compose-draft-loss`. What was dismissed from the outbox in the last days, read back and drawn. */
  loadDismissedSends(): unknown;
  renderDismissedSends(): unknown;
  /** Undo for a dismissed send and a removed bookmark, and every dismissed send forgotten. */
  undoSendDismiss(): unknown;
  undoBookmarkRemoval(): unknown;
  forgetDismissedSends(): unknown;
}

/** The listeners, mounted. */
export interface KeptSends {
  /** Take every listener down again. */
  unmount(): void;
}

/** Read back what the page kept of its sends, and mount the listeners over it, in the page's order. */
export function mountKeptSends(deps: KeptSendsDependencies): KeptSends {
  const { listen, unmount } = createListening();
  // `#51 reply-view`. Every way out of a reply goes through `closeReply`, so none can be the one that forgets to put
  // the reader back where they were reading.
  deps.loadReplyDrafts();
  deps.loadOwnPosts();
  deps.loadOutgoingMessages();
  // A page going hands its own over.
  deps.sendLeftOverInboxWrites();
  listen<Event>(window, "online", () => deps.sendLeftOverInboxWrites());
  listen<Event>(window, "pagehide", () => deps.leaveInbox());
  deps.loadDismissedSends();
  deps.renderDismissedSends();
  listen<Event>(deps.lookup("undo-send-dismiss"), "click", () => {
    deps.undoSendDismiss();
  });
  // `#420 bookmark-clear`.
  listen<Event>(deps.lookup("undo-bookmark"), "click", () => deps.undoBookmarkRemoval());
  listen<Event>(deps.lookup("forget-dismissed-sends"), "click", () => deps.forgetDismissedSends());
  return { unmount };
}

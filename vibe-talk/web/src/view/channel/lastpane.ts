// The pane the keys moved last (`#229 desktop-two-column`, `#281 view-keys`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel): keys pressed from outside every pane go to it while it
// takes them. Which panes there are, which takes keys now, and what moving them means are the page's
// (`paneFor`, `takeRow`).

/** The pane the keys moved last. */
export interface LastPane<Pane> {
  /** The pane the keys moved last. */
  current(): Pane;
  /** The keys moved `pane`. */
  take(pane: Pane): void;
}

/** The keys last moved `first`, as before any key has been pressed. */
export function createLastPane<Pane>(first: Pane): LastPane<Pane> {
  let lastPane = first;
  return {
    current: () => lastPane,
    take(pane) {
      lastPane = pane;
    },
  };
}

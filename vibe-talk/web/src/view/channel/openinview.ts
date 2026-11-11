// Which view a sent card's Open in channel lands in, and drawing the message links again
// when the switch that decides them changes (`#624 voice-page-modularity`, moved from
// the legacy script). Words and order are as they were in the script. The stored choice
// itself (its key on this device) and the rows are the page's, handed in.

/** What the open-in-view choice asks of the page. */
export interface OpenInViewDependencies {
  threading(): boolean;
  eachColumn(act: () => void): void;
  el(id: string): HTMLElement;
  renderCachedTimeline(): void;
  view(): string;
  threadCards(threads: readonly unknown[]): Iterable<Node>;
  threads(): readonly unknown[];
  renderSideColumn(): void;
  regroupChannelRows(redraw: boolean): void;
  storedChoice(): "flat" | "main";
  keepChoice(view: string): void;
  renderChoice(): void;
}

/** The open-in-view choice and the links redrawn for it. */
export interface OpenInView {
  redrawMessageLinks(): void;
  storedOpenInView(): "flat" | "main";
  renderOpenInView(): void;
  chooseOpenInView(view: string): void;
}

/** The open-in-view choice over `deps`. */
export function createOpenInView(deps: OpenInViewDependencies): OpenInView {
  /**
   * Draw again every body the switch decides the links of, from what is held, as for Empty messages: each
   * column's rows, the thread list's cards and the right column's.
   */
  function redrawMessageLinks(): void {
    // The links are wired as each body is built, so the rows are drawn again from nothing:
    // a kept row (`#1229 keyed-row-patching`) would keep the links the old switch gave it.
    if (deps.threading()) {
      deps.eachColumn(() => {
        deps.el("discord-log").replaceChildren();
        deps.renderCachedTimeline();
      });
      deps.eachColumn(() => {
        if (deps.view() === "threads") deps.el("thread-list").replaceChildren(...deps.threadCards(deps.threads()));
      });
      deps.renderSideColumn();
    } else {
      deps.eachColumn(() => deps.regroupChannelRows(true));
    }
  }

  /**
   * The view a sent card's Open in channel lands in (`#580 open-in-chat-jump`): "flat", All, every
   * message in the order sent, unless the reader chose "main", the threads. ABSENT MEANS ALL, the
   * owner's choice: "I suggest that the all view with all chronological messages be the default".
   *
   * @returns {"flat" | "main"}
   */
  function storedOpenInView(): "flat" | "main" {
    return deps.storedChoice();
  }

  /** Draw the choice as stored. Never writes it: an absent choice stays absent, so it syncs nothing. */
  function renderOpenInView(): void {
    deps.renderChoice();
  }

  /** Keep the reader's choice, `view`, and draw it. */
  function chooseOpenInView(view: string): void {
    try {
      deps.keepChoice(view);
    } catch (_error) {
      // A browser that refuses storage keeps the shipped default.
    }
    renderOpenInView();
  }

  return { redrawMessageLinks, storedOpenInView, renderOpenInView, chooseOpenInView };
}

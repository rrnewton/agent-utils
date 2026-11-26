// Saying that a read is on the wire, and whether a read was asked for by nothing the reader
// did (`#624 voice-page-modularity`, moved from the legacy script). Words and order are as
// they were in the script. How old the rows look is the freshness pill's, still in the
// script, asked to redraw here.

/** What saying a read is on the wire asks of the page. */
export interface ChannelLoadingDependencies {
  el(id: string): HTMLElement;
  loadState(): { refreshing: boolean };
  renderChannelFreshness(): void;
  selection(): { view: string };
}

/** The loading line and what counts as a background read. */
export interface ChannelLoading {
  renderChannelLoading(loading: boolean, quiet?: boolean, background?: boolean): void;
  backgroundRead(options?: { reason?: string | (string | undefined)[] }): boolean;
}

/** The channel's loading line over `deps`. */
export function createChannelLoading(deps: ChannelLoadingDependencies): ChannelLoading {
  /**
   * Say that a read is on the wire, or that none is. `quiet` is a read behind rows already on screen
   * for a view switch, `#220 view-switch-instant`: the pill says it is refreshing and the list shows
   * no loading line, which over rows the reader is already reading would only say "wait".
   *
   * `background` is a refresh nobody asked for over rows on screen — the poll, the read behind a live
   * message, coming back to the page — and it never puts the line up, however long it is out: the
   * pill says "refreshing…" all the while. The line stands in the scroller's flow above the history,
   * so putting it up and taking it down moved every row on screen by its height and back (27.7px at
   * 412px), the row being read included (`#2291 vibe-talk-top-anchor-arrival-27px-race`, rule 370);
   * until then a background read showed it once it had been out a second (`#226
   * page-energy-profile`). The line is for a read the reader asked for, and for a list with nothing
   * in it yet.
   */
  function renderChannelLoading(loading: boolean, quiet = false, background = false): void {
    const indicator = deps.el("channel-loading");
    deps.loadState().refreshing = loading;
    indicator.hidden = !loading || quiet || background;
    // "Saved 14:05" and "Saved 14:05 · refreshing…" are different claims; only the second waits.
    deps.renderChannelFreshness();
    if (!loading) return;
    indicator.textContent = deps.selection().view === "threads"
      ? "Loading threads…"
      : deps.selection().view === "thread"
        ? "Loading thread…"
        : "Loading messages…";
  }

  /**
   * Whether a read was asked for by nothing the reader did — the poll, a live message, coming back to
   * the page, the stream's replay or an edit it carried — every reason a queued read stands for
   * included. `#226 page-energy-profile`.
   *
   */
  function backgroundRead(options?: { reason?: string | (string | undefined)[] }): boolean {
    const reasons = options && Array.isArray(options.reason) ? options.reason : [options ? options.reason : undefined];
    return reasons.every((reason) => ["poll", "live", "visible", "replay", "mutation"].includes(String(reason)));
  }

  return { renderChannelLoading, backgroundRead };
}


// Saying that a read is on the wire, and whether a read was asked for by nothing the reader
// did (`#624 voice-page-modularity`, moved from the legacy script). Words and order are as
// they were in the script. How old the rows look is the freshness pill's, still in the
// script, asked to redraw here.

/** What saying a read is on the wire asks of the page. */
export interface ChannelLoadingDependencies {
  el(id: string): HTMLElement;
  loadState(): { refreshing: boolean; lineTimer: ReturnType<typeof setTimeout> | null };
  renderChannelFreshness(): void;
  selection(): { view: string };
}

/** How long a refresh nobody asked for may be out before its loading line shows: one second. `#226`. */
export const LOADING_LINE_DELAY_MS = 1000;

/** The loading line and what counts as a background read. */
export interface ChannelLoading {
  renderChannelLoading(loading: boolean, quiet?: boolean, delayed?: boolean): void;
  backgroundRead(options?: { reason?: string | (string | undefined)[] }): boolean;
}

/** The channel's loading line over `deps`. */
export function createChannelLoading(deps: ChannelLoadingDependencies): ChannelLoading {
  /**
   * Say that a read is on the wire, or that none is. `quiet` is a read behind rows already on screen
   * for a view switch, `#220 view-switch-instant`: the pill says it is refreshing and the list shows
   * no loading line, which over rows the reader is already reading would only say "wait".
   *
   * `delayed` is a refresh nobody asked for over rows on screen — the poll, the read behind a live
   * message, coming back to the page — and its line shows only if it is still out after
   * `LOADING_LINE_DELAY_MS`. `#226 page-energy-profile`: shown at once, the line and its spinner went
   * in above the whole history and came out again a moment later on every poll, laying the list out
   * twice and animating, which was most of what a refresh with nothing new still cost. The pill says
   * "refreshing…" all the while, and a slow refresh still shows the line.
   */
  function renderChannelLoading(loading: boolean, quiet = false, delayed = false): void {
    const indicator = deps.el("channel-loading");
    deps.loadState().refreshing = loading;
    if (deps.loadState().lineTimer !== null) {
      clearTimeout(deps.loadState().lineTimer as ReturnType<typeof setTimeout>);
      deps.loadState().lineTimer = null;
    }
    if (loading && delayed && !quiet && indicator.hidden) {
      deps.loadState().lineTimer = setTimeout(() => {
        deps.loadState().lineTimer = null;
        if (deps.loadState().refreshing) indicator.hidden = false;
      }, LOADING_LINE_DELAY_MS);
    } else {
      indicator.hidden = !loading || quiet;
    }
    // "Saved 14:05" and "Saved 14:05 · refreshing…" are different claims; only the second waits.
    deps.renderChannelFreshness();
    if (!loading) return;
    indicator.textContent = deps.selection().view === "threads"
      ? "Loading threads…"
      : deps.selection().view === "thread"
        ? "Loading thread…"
        : "Loading messages…";
  }

  // The timer that shows that line, or null. Kept per column: `deps.loadState().lineTimer` (`freshLoadState`, `#624 voice-page-modularity`).

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


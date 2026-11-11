// How old what is on screen is, said over the list and on the right column's line
// (`#624 voice-page-modularity`, moved from the legacy script as a pure move, cleared
// by name with the scroll owner). Words and order are as they were in the script, and
// the room is none (`#1320`: the pill floats over the list and reserves no band, so
// `#1210`'s no-shift holds with nothing held statically). The load state, the words
// for stale, and the place-keeping are the page's, handed in.

/** The load state, as freshness reads it. */
export interface FreshnessLoadState {
  freshness: string;
  freshAt: number;
  refreshing: boolean;
}

/** What saying how fresh the rows are asks of the page. */
export interface FreshnessDependencies {
  loadState(): FreshnessLoadState;
  side(): boolean;
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  stamp(at: number): string;
  liveAttached(): boolean;
  liveDelivery(): string;
  staleWords(state: string, at: number, refreshing: boolean, what: string, service: () => string): string;
  chatServiceOf(channel: unknown): string;
  selectedChannelId(): unknown;
  view(): string;
  searchOpen(): boolean;
  columnsShown(): boolean;
  sideColumn(): { mode: string };
  mainColumn(): unknown;
  inColumn<T>(column: unknown, act: () => T): T;
  sideColumnUp(): boolean;
  threadListFailure(channel: string): string;
  threadListLandedAt(channel: string): number;
  setTextIfChanged(node: HTMLElement, text: string): void;
  holdingReader(act: () => void): void;
  keepingCardPlace(act: () => void): void;
}

/** Setting and drawing how fresh the rows are. */
export interface Freshness {
  setChannelFreshness(state: string): void;
  renderChannelFreshness(): void;
  renderSideFreshness(): void;
}

/** Freshness over `deps`. */
export function createFreshness(deps: FreshnessDependencies): Freshness {
  function setChannelFreshness(state: string): void {
    deps.loadState().freshness = state;
    renderChannelFreshness();
  }

  /**
   * Over the top of the list rather than in it: a sentence at the head of the history scrolls away
   * with it, and the reader is usually at the bottom, which is exactly where "these are old" has to
   * be visible. Hidden while the pull-to-refresh pill is in the same place — a pull at the top. A
   * pull up from the newest line puts that pill at the foot of the list, and this one stays.
   */
  function renderChannelFreshness(): void {
    // The pill floats over the left column and speaks for it. `#229 desktop-two-column`: the right
    // column's freshness is its own, said on a line of that column (`#231 two-column-polish`).
    if (deps.side()) {
      renderSideFreshness();
      return;
    }
    const pill = deps.el("channel-freshness");
    const gesture = deps.el("pull-refresh");
    const at = deps.stamp(deps.loadState().freshAt);
    const refreshing = deps.loadState().refreshing;
    // CURRENT IS SAID TOO, not left blank: a blank pill reads the same as a page that never managed
    // to refresh, which is what a reader opening the app in the car could not tell apart.
    const current = deps.loadState().freshAt
      ? `${deps.liveAttached() && deps.liveDelivery() !== "off" ? "Live · updated" : "Updated"} ${at}` +
        (refreshing ? " · refreshing…" : "")
      : "";
    const text = deps.staleWords(deps.loadState().freshness, deps.loadState().freshAt, refreshing, "messages", () => deps.chatServiceOf(deps.selectedChannelId())) || current;
    pill.textContent = text;
    pill.setAttribute("data-state", deps.loadState().freshness);
    // It gives way to a pull at the TOP, which draws in the same place (a pull up at the foot leaves
    // "Offline" or "Refresh failed" standing, `#188 pull-refresh-bottom`), and — since `#197
    // floating-search` put the search glass on the same line — to the open search bar, which grows
    // along that line across the top of the list.
    pill.hidden = text === "" || deps.view() !== "discord" ||
      (!gesture.hidden && gesture.getAttribute("data-edge") !== "end") || deps.searchOpen();
    // No room is reserved or released here, or anywhere (#1320, keeping #1210): the pill floats
    // over the list on a zero-height anchor and takes no layout room, so the scroller never
    // resizes and no row moves as freshness appears, clears, or is reworded. Callers that land
    // a row measure the pill as drawn (`floatingClearance`), which is unchanged.
  }

  /**
   * The right column's line saying how old its rows are: drawn only when it changed, and only when
   * they are not current. `#231 two-column-polish`. A thread says what its own reads have said, as
   * the pill says Main's; the cards say what the thread list's reads have said, by the newest that
   * landed — else by the left column's, whose store the cards are drawn from — and nothing once one
   * lands again.
   */
  function renderSideFreshness(): void {
    const line = deps.leftEl("side-freshness");
    let state = "fresh";
    let text = "";
    if (deps.columnsShown() && deps.sideColumn().mode === "thread") {
      [state, text] = deps.inColumn(deps.sideColumn(), () =>
        [deps.loadState().freshness, deps.staleWords(deps.loadState().freshness, deps.loadState().freshAt, deps.loadState().refreshing, "messages", () => deps.chatServiceOf(deps.selectedChannelId()))]);
    } else if (deps.sideColumnUp()) {
      const channel = String(deps.selectedChannelId());
      state = deps.threadListFailure(channel) || "fresh";
      const at = deps.threadListLandedAt(channel) || deps.inColumn(deps.mainColumn(), () => deps.loadState().freshAt);
      // With no time to give, the state alone, rather than a time nothing was read at.
      text = state === "fresh" ? "" : at ? deps.staleWords(state, at, false, "threads", () => deps.chatServiceOf(deps.selectedChannelId()))
        : state === "offline" ? "Offline" : "Refresh failed";
    }
    deps.setTextIfChanged(line, text);
    if (line.getAttribute("data-state") !== state) line.setAttribute("data-state", state);
    const hide = text === "";
    if (line.hidden === hide) return;
    // The line's height comes out of the list's below it, which browser scroll anchoring does not
    // hold: a reader of the thread keeps the line they are on, or the newest line (`holdingReader`).
    const toggle = () => {
      line.hidden = hide;
    };
    if (deps.columnsShown() && deps.sideColumn().mode === "thread") deps.inColumn(deps.sideColumn(), () => deps.holdingReader(toggle));
    // `#261 two-column-connectors`: and a reader of the cards keeps theirs, the foot above all.
    else deps.keepingCardPlace(toggle);
  }

  return { setChannelFreshness, renderChannelFreshness, renderSideFreshness };
}

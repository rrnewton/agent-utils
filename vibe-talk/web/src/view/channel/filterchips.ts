// The search and the filter chips' listeners (`#129 message-search`, `#206 pin-message`, `#211 link-filter`, `#212
// link-filter`, `#251 author-filter`, `#266 link-filter-kinds`, `#620 link-url-chip`, `#249 search-highlight`,
// `#262 search-load-older`; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/channel for
// the plan's step 17): the glass and the filters as toggles, each kind of link, the search field's typing, Enter and
// Escape. What each does is the page's; mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";
import { LINK_KINDS } from "../../present/rows/index.ts";

/** A keydown in the search field, as far as its listener reads it. */
export interface SearchKey {
  readonly key?: string;
  readonly isComposing?: boolean;
  preventDefault?(): void;
}

export interface FilterChipsDependencies {
  /** Bring an element into view through the scroll owner (`#1251 scroll-single-owner`). */
  intoView(element: Element, block: string, inline?: string): void;
  /** An element of the page by id, in the column the code is working in. */
  byId(id: string): HTMLElement;
  /** Whether the search is open, and opening or closing it (closing clears it). */
  searchOpen(): boolean;
  setSearchOpen(open: boolean): void;
  /** Whether only the pinned messages are shown, and showing only them or all again. */
  pinnedOnly(): boolean;
  setPinnedOnly(on: boolean): void;
  /** Whether only the messages with links are shown, and showing only them or all again. */
  linksOnly(): boolean;
  setLinksOnly(on: boolean): void;
  /** Everyone, Mine, Theirs, in turn. */
  cycleAuthorFilter(): void;
  /** Whether a kind of link is hidden, and showing or hiding it. */
  linkKindHidden(kind: string): boolean;
  setLinkKindShown(kind: string, shown: boolean): void;
  /** Whether links are shown as their addresses, and showing them so or by name. */
  linksRaw(): boolean;
  setLinksRaw(raw: boolean): void;
  /** Run `change`, the reader's message kept where it is on the screen (`#243 filter-keeps-selection`). */
  keepingFilterPlace(change: () => void): void;
  /** Run `change`, Hide read stepping aside or coming back as the search's terms come and go (`#262`). */
  liftingHideRead(change: () => void): void;
  /** The search's words: set from the field, read, and the words whose hits are marked. */
  setQuery(text: string): void;
  query(): string;
  setMarked(text: string): void;
  /** Whether `text` has a word to find. */
  searchTyped(text: string): boolean;
  /** Draw every column's chips again. */
  renderEveryColumnsTools(): void;
  /** Whether the right column is drawn, and filtering its cards again. */
  sideColumnUp(): boolean;
  refilterSideCards(): void;
  /** Mark the hits once the typing pauses, or now. */
  markSearchSoon(): void;
  markSearchNow(): void;
  /** A search ran with `term`: hold it for Up to recall (`#1816`). */
  rememberSearch(term: string): void;
  /** Up: the term one step older, holding `current` as the draft, or null (`#1816`). */
  historyOlder(current: string): string | null;
  /** Down: the term one step newer, or the draft past the newest, or null (`#1816`). */
  historyNewer(): string | null;
  /** The reader typed: any history walk is over (`#1816`). */
  historyReset(): void;
  /** Search further back from the field, where the step back offers it; whether it did. */
  searchOlderFromField(): boolean;
  /** Note which rows stand on screen before a typed search filters (`#1734`). */
  noteSearchScreen(): void;
  /** Land a newly typed search (`#1734`): stay on a visible hit, else go to the newest hits. */
  landSearchEntry(): void;
  /** Give the list on screen the focus, unless the reader is typing in a field still on screen. */
  settleFocus(): void;
  /** Keep a key from the browser's own default for it. */
  swallow(event: SearchKey): void;
}

/** The listeners, mounted. */
export interface FilterChips {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount the search's and the filter chips' listeners. */
export function mountFilterChips(deps: FilterChipsDependencies): FilterChips {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  // `#129 message-search`. The glass is a toggle, and the second tap is the way out: a control that
  // only ever opens leaves the reader looking for a close button that is not on a phone keyboard.
  listen(el("search-toggle"), "click", () => deps.setSearchOpen(!deps.searchOpen()));
  // `#206 pin-message`. A toggle, like the glass beside it.
  listen(el("pinned-filter"), "click", () => deps.setPinnedOnly(!deps.pinnedOnly()));
  // `#211 link-filter`. The same, for links.
  listen(el("links-filter"), "click", () => deps.setLinksOnly(!deps.linksOnly()));
  // `#251 author-filter`. Everyone, Mine, Theirs, in turn.
  listen(el("author-filter"), "click", () => deps.cycleAuthorFilter());
  // `#212 link-filter`. And each kind of link under it, a toggle of its own.
  for (const { kind, chip } of LINK_KINDS) {
    listen(el(chip), "click", () => deps.setLinkKindShown(kind, deps.linkKindHidden(kind)));
  }
  // `#620 link-url-chip`. URL, first in that row: addresses or names.
  listen(el("link-raw"), "click", () => deps.setLinksRaw(!deps.linksRaw()));
  // `#266 link-filter-kinds`. The row of six scrolls sideways on a phone, and a browser scrolls it to a
  // button taking the focus only when none of that button shows: one cut off at the row's end stayed cut
  // off under Tab. So the button the focus reaches is brought wholly into the row, by Tab or by a tap.
  listen<FocusEvent>(el("link-kinds"), "focusin", (event) => {
    const target = event.target as HTMLElement | null;
    if (target) deps.intoView(target, "nearest", "nearest");
  });
  // Typed or cleared — by the keyboard, the field's own clear button, or a paste — the text is a filter
  // change, and the reader's message stays where it is on the screen (`#243 filter-keeps-selection`).
  // `#212 link-filter`, from review: text that matches nothing takes the reader to the sentence saying
  // so, and text that matches again puts back the message they were on before it.
  // The field's text changed — typed, or a history term recalled into it (`#1816`): the one
  // filter change both arrive as, so a recalled term filters exactly as if typed.
  function fieldChanged(): void {
    // `#1734 vibe-talk-search-view-scroll-default`. Which rows stand on screen
    // is read before the filter moves them: "on screen" names the screen the
    // reader was looking at when they typed, not the compacted list after.
    deps.noteSearchScreen();
    // `#262 search-load-older`. The first term typed, or the last taken away, may make Hide read step
    // aside or come back (`liftingHideRead`).
    deps.keepingFilterPlace(() => deps.liftingHideRead(() => {
      deps.setQuery((el("search-field") as HTMLInputElement).value);
      // `#249 search-highlight`. Cleared — no word left to find — the marks go with the words, in this
      // one redraw; otherwise they follow once the typing pauses (`searchField.marked()`).
      if (!deps.searchTyped(deps.query())) deps.setMarked(deps.query());
      deps.renderEveryColumnsTools();
      if (deps.sideColumnUp()) deps.refilterSideCards();
    }));
    // `#1734 vibe-talk-search-view-scroll-default`. After the place-keeper has
    // put the reader back, a newly entered search lands: on a hit already on
    // screen nothing moves; with none on screen the reader goes to the foot,
    // where the most recent hits stand.
    deps.landSearchEntry();
    deps.markSearchSoon();
  }
  listen(el("search-field"), "input", () => {
    // `#1816 vibe-talk-search-history-up-arrow`. Typing ends any history walk: what stands in
    // the field now is the draft the next Up holds.
    deps.historyReset();
    fieldChanged();
  });
  // Escape closes AND clears, which is the same act — see `setSearchOpen`. It is the gesture a
  // desktop reader reaches for without being told, and the only one a keyboard has. `#224
  // keyboard-shortcuts`: and the reader is put back on the list, rather than on a field that has just
  // gone; handled here, so the page's own listener (`keyDispatch.onPageKey`) leaves it alone. Not while an input
  // method composes, whose Escape is its own.
  listen<KeyboardEvent>(el("search-field"), "keydown", (event) => {
    // `#249 search-highlight`. Enter — a phone keyboard's Search — is the reader saying the words are
    // typed: the hits are marked now rather than when the pause would have.
    if (event.key === "Enter" && !event.isComposing) {
      deps.markSearchNow();
      // `#1816 vibe-talk-search-history-up-arrow`. Enter runs the search: its words are held
      // for Up to recall, newest first.
      deps.rememberSearch((el("search-field") as HTMLInputElement).value);
      // `#262 search-load-older`. And it searches further back, where the step back offers that.
      if (deps.searchOlderFromField()) deps.swallow(event);
    }
    // `#1816 vibe-talk-search-history-up-arrow`. Up and Down walk the terms run before, like
    // shell history: Up holds what is typed as the draft, Down past the newest term gives it
    // back. The key is swallowed only when a term (or the draft) actually lands in the field,
    // so with nothing to recall the caret keeps its own meaning for the key. A recalled term
    // is applied as if typed, through the one filter change.
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.isComposing) {
      const field = el("search-field") as HTMLInputElement;
      const term = event.key === "ArrowUp" ? deps.historyOlder(field.value) : deps.historyNewer();
      if (term !== null) {
        field.value = term;
        fieldChanged();
        deps.swallow(event);
      }
    }
    if (event.key === "Escape" && !event.isComposing) {
      deps.historyReset();
      deps.setSearchOpen(false);
      deps.settleFocus();
      deps.swallow(event);
    }
  });
  return { unmount };
}

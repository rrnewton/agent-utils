// The search bar opened and closed (`#624 voice-page-modularity`, moved from the legacy
// script): closing it clears the query and the filters that live in it, inside the
// filter-kept place. Words and order are as they were in the script. The field, the
// place-keeping and the renders are the page's, handed in.

import type { ElementById } from "../dom/index.ts";
import type { SearchFieldLike } from "./searchrows.ts";

/** The search field, as opening and closing it uses it. */
export interface SearchFieldControl extends SearchFieldLike {
  setOpen(open: boolean): void;
  setQuery(query: string): void;
  setMarked(marked: string): void;
}

/** What opening and closing the search bar asks of the page. */
export interface SearchOpenDependencies {
  keepingFilterPlace(act: () => void): void;
  searchField: SearchFieldControl;
  markSearchSoon(): void;
  el<K extends string>(id: K): K extends keyof ElementById ? ElementById[K] : HTMLElement;
  setLinksOnly(on: boolean): void;
  setPinnedOnly(on: boolean): void;
  setAuthorFilter(author: string): void;
  hidingRead(): boolean;
  holdingReader(act: () => void): void;
  renderControlBar(): void;
  renderChannelFreshness(): void;
  eachColumn(act: () => void): void;
  renderCachedTimeline(): void;
  renderScrollTools(): void;
  sideColumnUp(): boolean;
  refilterSideCards(): void;
}

/** The search bar opened and closed. */
export interface SearchOpen {
  setSearchOpen(open: boolean): void;
  showSearchOpen(open: boolean): void;
}

/** The search bar's opening and closing over `deps`. */
export function createSearchOpenControl(deps: SearchOpenDependencies): SearchOpen {
  function setSearchOpen(open: boolean): void {
    if (!open) {
      deps.keepingFilterPlace(() => showSearchOpen(false));
      return;
    }
    showSearchOpen(true);
  }

  /** `setSearchOpen`, without holding the reader's place. */
  function showSearchOpen(open: boolean): void {
    // `#262 search-load-older`. Closed over a filter choosing rows, Hide read comes back (`hidingRead`).
    const wasHidingRead = deps.hidingRead();
    deps.searchField.setOpen(open);
    if (!open) {
      deps.searchField.setQuery("");
      // `#249 search-highlight`. And its marks, at once.
      deps.searchField.setMarked("");
      deps.markSearchSoon();
      deps.el("search-field").value = "";
      // `#206 pin-message`. The Pinned filter lives in this bar, and goes with it for the reason the
      // query does: a filter left on behind a control that is no longer on screen is the state this
      // feature must never be in. `#211 link-filter`: so does Links. Placed once, for the whole of
      // closing, by `keepingFilterPlace`.
      deps.setLinksOnly(false);
      deps.setPinnedOnly(false);
      // `#251 author-filter`. And whose messages.
      deps.setAuthorFilter("everyone");
    }
    deps.el("search-toggle").setAttribute("aria-pressed", open ? "true" : "false");
    // `#197 floating-search`. The open bar floats over the top of the list, and web/voice.css keys
    // two things off this: the bar's own shape, and the room the list makes for it at its head so
    // the first match is never underneath. That room is added ABOVE a reader who may have scrolled
    // down, which is exactly the change `holdingReader` exists for.
    deps.holdingReader(() => {
      if (open) deps.el("screen-main").setAttribute("data-searching", "");
      else deps.el("screen-main").removeAttribute("data-searching");
    });
    deps.renderControlBar();
    // The pill shares the line the bar grows along, and gives way to it.
    deps.renderChannelFreshness();
    // Both columns: there is one search bar. `#229 desktop-two-column`.
    if (deps.hidingRead() !== wasHidingRead) deps.eachColumn(deps.renderCachedTimeline);
    deps.eachColumn(deps.renderScrollTools);
    if (deps.sideColumnUp()) deps.refilterSideCards();
    if (open) {
      deps.el("search-field").focus();
    }
  }

  return { setSearchOpen, showSearchOpen };
}

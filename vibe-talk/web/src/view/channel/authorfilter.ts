// The author filter in the search bar (`#251 author-filter`, `#520 search-whose-toggle-hint`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel): which rows are the owner's,
// whether a row passes, and the one owner of the filter chosen, its button and the chip that names
// where a press landed. Its words are web/src/present/rows's; keeping the reader's place while it
// changes, and the one pass that applies every filter (`applySearch`), are the page's.

import { AUTHOR_FILTER_CHIP_LABELS, AUTHOR_FILTER_LABELS, AUTHOR_FILTER_TITLES } from "../../present/rows/index.ts";

/** How long the chip stays up before it fades, in ms. Its fade is web/voice.css's. */
const AUTHOR_CHIP_MS = 1500;

/**
 * Whether `row` is a message the owner wrote, by what the page draws: a channel row in the `me` bucket
 * (or one still being sent, which is his), and the call's "you".
 */
function rowIsMine(row: Element): boolean {
  if (row.getAttribute("data-who") === "me") return true;
  return String(row.className).split(/\s+/).includes("mine");
}

/**
 * Whether `row` passes the author filter `author`. Only a message passes Mine or Theirs: a row that is
 * not one (a notice, a seam, a card of the Threads list, which is a thread and not a message anybody
 * wrote) goes, as it does under Links.
 */
function authorPasses(row: Element, author: string): boolean {
  if (author === "everyone") return true;
  if (row.getAttribute("data-search") === null || String(row.className).split(/\s+/).includes("thread-card")) {
    return false;
  }
  return rowIsMine(row) === (author === "mine");
}

/** What the filter asks of the page. */
export interface AuthorFilterDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** Whether the search bar is open (`searchOpen`). */
  searchOpen(): boolean;
}

/** The filter's commands and questions. */
export interface AuthorFilter {
  /** Whose messages the list shows: one of `AUTHOR_FILTERS`, as last chosen. */
  chosen(): string;
  /** The author filter in force: "everyone" while the bar is shut, whatever was last chosen. */
  inForce(): string;
  /** Choose `mode` and say so on the button. */
  set(mode: string): void;
  /** The button, saying the state. */
  render(): void;
  /** The chip under the button naming the state a press landed on. */
  showChip(): void;
}

/** Everyone, and no chip. */
export function createAuthorFilter(deps: AuthorFilterDependencies): AuthorFilter {
  /** The chip's fade timer and its hide timer; a fresh press restarts both. */
  let authorChipFadeTimer: ReturnType<typeof setTimeout> | null = null;
  let authorChipHideTimer: ReturnType<typeof setTimeout> | null = null;

  /** Whose messages the list shows: one of `AUTHOR_FILTERS`. Only while the search bar is open. */
  let authorFilter = "everyone";

  /** The button, saying the state: its icon (drawn by web/voice.css off `data-author`), pressed, and named. */
  function renderAuthorFilter(): void {
    const button = deps.lookup("author-filter");
    const said: Record<string, string> = {
      "data-author": authorFilter,
      "aria-pressed": authorFilter === "everyone" ? "false" : "true",
      "aria-label": AUTHOR_FILTER_LABELS[authorFilter] as string,
      "aria-describedby": "author-filter-tip",
      title: AUTHOR_FILTER_TITLES[authorFilter] as string,
    };
    for (const [name, value] of Object.entries(said)) {
      if (button.getAttribute(name) !== value) button.setAttribute(name, value);
    }
    // `#520 search-whose-toggle-hint`. The hover/focus tooltip says the same as the title.
    const tip = deps.lookup("author-filter-tip");
    if (tip.textContent !== AUTHOR_FILTER_TITLES[authorFilter]) {
      tip.textContent = AUTHOR_FILTER_TITLES[authorFilter] as string;
    }
  }

  /**
   * `#520 search-whose-toggle-hint`. Leave the chip under the button naming the state a press landed
   * on, for `AUTHOR_CHIP_MS`, then fade it (web/voice.css; no fade under reduced motion). It floats,
   * so the bar and the list do not move. A fresh press restarts the wait.
   */
  function showAuthorChip(): void {
    const chip = deps.lookup("author-filter-chip");
    const fading = (on: boolean): void => {
      const names = String(chip.className).split(/\s+/).filter((name) => name && name !== "author-chip-out");
      chip.className = on ? [...names, "author-chip-out"].join(" ") : names.join(" ");
    };
    chip.textContent = AUTHOR_FILTER_CHIP_LABELS[authorFilter] as string;
    fading(false);
    chip.hidden = false;
    if (authorChipFadeTimer !== null) clearTimeout(authorChipFadeTimer);
    if (authorChipHideTimer !== null) clearTimeout(authorChipHideTimer);
    authorChipFadeTimer = setTimeout(() => {
      authorChipFadeTimer = null;
      fading(true);
      authorChipHideTimer = setTimeout(() => {
        authorChipHideTimer = null;
        chip.hidden = true;
        fading(false);
      }, 400);
    }, AUTHOR_CHIP_MS);
  }

  return {
    chosen: () => authorFilter,
    inForce: () => (deps.searchOpen() ? authorFilter : "everyone"),
    set(mode) {
      authorFilter = mode;
      renderAuthorFilter();
    },
    render: renderAuthorFilter,
    showChip: showAuthorChip,
  };
}

// What the page and the tests use of it.
export { AUTHOR_CHIP_MS, authorPasses, rowIsMine };

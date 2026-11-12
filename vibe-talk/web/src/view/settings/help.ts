// Help, folded (`#246 help-sections`; `#624 voice-page-modularity`, moved from the legacy script; plan
// step 16, view/settings).
//
// The owner: "when I press "?" I jump to one section of one giant help page. ... make them collapsable
// and all folded except the one that we click on to jump to help."
//
//   * EACH ENTRY IS A NATIVE <details> (web/voice.html), its heading the <summary>: a button to a
//     screen reader, Enter and Space from the keyboard, a tap on a phone, with nothing written here
//     to make it one.
//   * FOLDED, EXCEPT THE ONE ASKED FOR. A settings group's ? opens its entry; "?" and Ctrl+? (Cmd+?)
//     open Keyboard, the entry about the keys (`#224`); Open help at the head of Settings is about
//     nothing in particular and opens none, so the screen is a list of what there is to read.
//   * ONE BUTTON AT THE TOP opens every entry, and once they are all open folds them all. It names
//     what it will do, like every toggle on this page that is an act rather than a mode.
//   * FIND STILL FINDS. A browser that searches folded <details> (Chromium does, and so does Firefox)
//     opens the entry its match is in and fires `toggle`, which keeps the button's word right. One
//     that does not cannot find text in a folded entry: Expand all first.
//
// Which screen is up, and the way to Help and back from it, are the page's.

import { foldAllWords, helpEntrySlugs } from "../../present/settings/index.ts";

/** What Help's folds ask of the page. */
export interface HelpFoldsDependencies {
  /** Bring an element into view through the scroll owner (`#1251 scroll-single-owner`; the owner calls it defensively, as this did). */
  intoView(element: Element, block: string, inline?: string): void;
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** The element with this id, as the document finds it (`document.getElementById`). */
  byId(id: string): HTMLElement | null;
}

/** Help's folds: their read surface and named commands. */
export interface HelpFolds {
  /**
   * Every entry of the Help screen: the ones a ? reaches, and the two read from the screen itself. By
   * id, as everything on this page is found, from the list the suite holds to the markup in both
   * directions.
   */
  entries(): HTMLDetailsElement[];
  /** The button at the top: Expand all while any entry on the screen is folded, Fold all once none is. */
  render(): void;
  /** Every entry folded but `slug`'s, which is brought into view; none open for a null slug. */
  openAt(slug: string | null): void;
}

/** The button and every entry listened to. Made once, where the page wires its Help. */
export function mountHelpFolds(deps: HelpFoldsDependencies): HelpFolds {
  const entries = (): HTMLDetailsElement[] => helpEntrySlugs().map(
    (slug) => deps.byId(`help-${slug}`) as HTMLDetailsElement
  );

  /**
   * The entries on the screen. One hidden with the feature it explains is not, and must not hold the
   * button at Expand all while every entry the reader can see is open: `#208 persistent-cache` hides
   * the one about saved messages on a server that has no message cache.
   */
  const shownEntries = (): HTMLDetailsElement[] => entries().filter((entry) => !entry.hidden);

  function render(): void {
    const shown = shownEntries();
    deps.lookup("fold-help").textContent = foldAllWords(shown.some((entry) => !entry.open));
  }

  deps.lookup("fold-help").addEventListener("click", () => {
    const shown = shownEntries();
    const open = shown.some((entry) => !entry.open);
    for (const entry of shown) entry.open = open;
    render();
  });
  for (const entry of entries()) entry.addEventListener("toggle", render);

  return {
    entries,
    render,
    /**
     * `#246 help-sections`. EVERY TIME, not only the first: Help is reached to read one thing, and the
     * entries opened on an earlier visit would put the screen back to the one long page this replaces.
     *
     * `scrollIntoView` is called defensively because the page's test fixture models the DOM the page is
     * allowed to use and a layout method is not part of that — the SCREEN still changes there, which is
     * the behaviour worth asserting, and scrolling is a thing only a real browser can do anyway.
     */
    openAt(slug) {
      for (const entry of entries()) entry.open = Boolean(slug) && entry.id === `help-${slug}`;
      render();
      if (!slug) {
        return;
      }
      const entry = deps.byId(`help-${slug}`);
      if (entry) deps.intoView(entry, "start");
    },
  };
}

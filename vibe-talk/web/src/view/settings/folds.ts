// Settings, folded (`#346 settings-collapsible`; `#624 voice-page-modularity`, moved from the legacy
// script; plan step 16, view/settings).
//
// The owner, 2026-10-18: "every top-level section collapsible, collapsed by default; its H2 subsections
// collapsed too; find-in-page still finds hidden text". Each fold is a button in a heading
// (`#fold-<slug>`) over a body (`#fold-<slug>-body`) that is `hidden="until-found"` while folded, so the
// browser's find still searches it and opens it on a match, firing `beforematch`, which is when the
// button is told. A section's fold and the groups' folds inside it are separate: a section opens to its
// groups' headings, each folded, and a match inside a group opens both.
//
// FOLDED ON EVERY LOAD, the markup's state and this owner's, and as the reader leaves them while the
// page is open: going to Help and back, or out of Settings and in again, keeps what is open. The state
// is the markup's own (each button's `aria-expanded`, each body's `hidden`); nothing else is kept.

import { FOLDS_HOLDING_DISMISSED, SETTINGS_FOLDS, foldAllWords } from "../../present/settings/index.ts";

/** What the folds ask of the page. */
export interface SettingsFoldsDependencies {
  /** Bring an element into view through the scroll owner (`#1251 scroll-single-owner`). */
  intoView(element: Element, block: string, inline?: string): void;
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** Close the pop-out over Recently dismissed, leaving the focus where it is (`closeDismissedSend(false)`). */
  closeDismissed(): void;
}

/** The folds' read surface and named commands. */
export interface SettingsFolds {
  /** Whether the fold `slug` is open. */
  open(slug: string): boolean;
  /** Open or fold `slug`, and redraw the button over them all. */
  set(slug: string, open: boolean): void;
  /** Whether the fold `slug` is on the page: no element around it hidden, up to Settings, but the folds. */
  shown(slug: string): boolean;
  /** Redraw the button over them all: Expand all while a fold on the page is folded, Fold all once none is. */
  renderAll(): void;
}

/** Every fold folded, and each button and body listened to. Made once, where the page wires its Settings. */
export function mountSettingsFolds(deps: SettingsFoldsDependencies): SettingsFolds {
  /** The folds on the page now (`shown`): the ones Expand all and Fold all are about. */
  const shownFolds = (): string[] => SETTINGS_FOLDS.filter((slug) => owner.shown(slug));

  const owner: SettingsFolds = {
    open: (slug) => deps.lookup(`fold-${slug}`).getAttribute("aria-expanded") === "true",
    /**
     * Its body shown, or hidden until found, and its button saying which. The body's attribute is
     * written rather than its `hidden` property, which a browser that knows `until-found` reads as the
     * string and one that does not as plain hidden.
     */
    set(slug, open) {
      const body = deps.lookup(`fold-${slug}-body`);
      if (open) body.removeAttribute("hidden");
      else body.setAttribute("hidden", "until-found");
      deps.lookup(`fold-${slug}`).setAttribute("aria-expanded", open ? "true" : "false");
      owner.renderAll();
    },
    /**
     * The folds around it aside (one inside a folded section is a fold Expand all opens), up to
     * Settings, which is hidden whenever another screen shows.
     *
     * `#351 settings-polish`. Two groups are hidden where there is nothing to show — Provider without a
     * console to link to, the message cache without one to manage — and their folds were counted all the
     * same: with every fold the reader can see open, the button still offered to expand them all.
     */
    shown(slug) {
      for (let at = deps.lookup(`fold-${slug}`).parentNode as HTMLElement | null;
        at && at.id !== "screen-settings"; at = at.parentNode as HTMLElement | null) {
        if (at.hidden === true && !/^fold-.+-body$/.test(at.id)) return false;
      }
      return true;
    },
    /**
     * As Help's button does (`#246`). Redrawn by every fold, and by a group shown or hidden (the
     * page's `renderAgentLink`, `renderServerCache`), which can bring a folded fold into sight or take
     * one out of it.
     */
    renderAll() {
      deps.lookup("fold-settings-all").textContent = foldAllWords(shownFolds().some((slug) => !owner.open(slug)));
    },
  };

  for (const slug of SETTINGS_FOLDS) {
    owner.set(slug, false);
    deps.lookup(`fold-${slug}`).addEventListener("click", () => {
      owner.set(slug, !owner.open(slug));
      if (!owner.open(slug) && FOLDS_HOLDING_DISMISSED.includes(slug)) deps.closeDismissed();
      // `#345 settings-multi-column`. In columns, a section opened or folded changes how they balance,
      // and can move to another column: the heading just pressed is brought back on screen, by as
      // little as it takes, so the reader does not lose what they opened.
      const pressed = deps.lookup(`fold-${slug}`);
      deps.intoView(pressed, "nearest");
    });
    // The browser's find (or a link to a text fragment) is opening the body: it removes `hidden`
    // itself, and the button has to say so.
    deps.lookup(`fold-${slug}-body`).addEventListener("beforematch", () => owner.set(slug, true));
  }
  deps.lookup("fold-settings-all").addEventListener("click", () => {
    // Decided by the folds on the page, as the button's words are; done to every fold, so a group shown
    // later opens or stays folded with the rest.
    const open = shownFolds().some((slug) => !owner.open(slug));
    for (const slug of SETTINGS_FOLDS) owner.set(slug, open);
    if (!open) deps.closeDismissed();
  });
  return owner;
}

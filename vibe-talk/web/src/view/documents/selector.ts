// The three-way view selector for the main screen's switch (`#view-switch`): the button names the
// view being shown, a press cycles Voice -> Channel -> Documents -> Voice, and two small cards
// stacked above the button in `#view-cards` name the other views in the order they come next, each
// choosing its view directly. Construction performs no effects: no element is looked up, no listener
// is added and no timer is started until `sync`, `cycle` or `toggleCards` is called.

import { createListening } from "../dom/index.ts";
import type { Listening } from "../dom/index.ts";

/** The three views the switch rotates through. */
export type DocViewName = "voice" | "discord" | "docs";

/** The views in rotation order: Voice, then Channel, then Documents, then Voice again. */
export const VIEW_ORDER: readonly DocViewName[] = Object.freeze(["voice", "discord", "docs"]);

/** The word shown for each view: on the switch for the current view, and on a card for the others. */
export const VIEW_LABELS: Readonly<Record<DocViewName, string>> = Object.freeze({
  voice: "Voice",
  discord: "Channel",
  docs: "Documents",
});

/** The view after `current` in the rotation: voice -> discord -> docs -> voice. */
export function nextView(current: DocViewName): DocViewName {
  const index = VIEW_ORDER.indexOf(current);
  return VIEW_ORDER[(index + 1) % VIEW_ORDER.length] ?? "voice";
}

/**
 * The two views that are not `current`, in the order they come next when rotating, so the cards
 * show what is coming next: for Voice, Channel then Documents; and so on around the rotation.
 */
export function otherViews(current: DocViewName): [DocViewName, DocViewName] {
  const next = nextView(current);
  return [next, nextView(next)];
}

/** What the selector needs from the page. */
export interface ViewSelectorDependencies {
  /** An element of the page by id: `view-switch`, `view-switch-label` and `view-cards`. */
  byId(id: string): HTMLElement;
  /** The view being shown now. */
  current(): DocViewName;
  /** Show `view`. */
  choose(view: DocViewName): unknown;
}

/** The view selector, once created. */
export interface ViewSelector {
  /**
   * Draw the selector for `current()`: the switch's word, title and accessible name, and the two
   * cards for the other views. The cards are hidden unless they have been shown (see `cycle` and
   * `toggleCards`). The first call also wires the switch's and the cards' clicks.
   */
  sync(): void;
  /** Choose the next view in the rotation, and show the cards for the view that lands. */
  cycle(): void;
  /** Show the cards if they are hidden, and hide them if they are shown. */
  toggleCards(): void;
  /** Hide the cards (the page calls this once they have been seen). */
  hideCards(): void;
  /** Take down every listener the selector added. */
  unmount(): void;
}

function setAttributeIfPresent(element: HTMLElement, name: string, value: string): void {
  if (typeof element.setAttribute === "function") element.setAttribute(name, value);
}

function clearChildren(element: HTMLElement): void {
  if (typeof element.replaceChildren === "function") {
    element.replaceChildren();
    return;
  }
  if (typeof element.removeChild === "function") {
    while (element.firstChild) element.removeChild(element.firstChild);
    return;
  }
  element.textContent = "";
}

function appendChild(parent: HTMLElement, child: HTMLElement): void {
  if (typeof parent.appendChild === "function") {
    parent.appendChild(child);
    return;
  }
  if (typeof parent.append === "function") parent.append(child);
}

/** Create the view selector. No effects happen here; they wait for an explicit call. */
export function createViewSelector(deps: ViewSelectorDependencies): ViewSelector {
  let cardsShown = false;
  let listening: Listening | null = null;

  const ensureListening = (): Listening => {
    if (!listening) listening = createListening();
    return listening;
  };

  function cycle(): void {
    deps.choose(nextView(deps.current()));
    cardsShown = true;
    sync();
  }

  function sync(): void {
    const current = deps.current();
    const label = VIEW_LABELS[current];
    const button = deps.byId("view-switch");

    const labelElement = deps.byId("view-switch-label");
    labelElement.textContent = label;

    const title = `Showing ${label}. Press to show ${VIEW_LABELS[nextView(current)]}.`;
    button.title = title;
    setAttributeIfPresent(button, "title", title);
    // The accessible name stays "Voice view" in every view, as the two-way switch kept it:
    // a name that changed with the view would hear "Channel, not pressed" as the channel off.
    setAttributeIfPresent(button, "aria-expanded", String(cardsShown));
    setAttributeIfPresent(button, "aria-controls", "view-cards");

    // The button's own press is the page's (`pressViewSwitch` in voice.js, wired by
    // the reply-box mount after `#624` batch 78); it calls `cycle()`. Wiring it here
    // too would cycle twice per press, so only the cards are wired here.
    const { listen } = ensureListening();

    const container = deps.byId("view-cards");
    container.hidden = !cardsShown;
    clearChildren(container);

    const doc = button.ownerDocument ?? (typeof document !== "undefined" ? document : null);
    if (!doc) return;

    for (const view of otherViews(current)) {
      // A card, not a <button>: the bar's own buttons are counted and laid out as the
      // dock's controls, and these two are a pop-up over it, not members of it.
      const card = doc.createElement("div");
      card.className = "view-card";
      if (typeof card.classList?.add === "function") card.classList.add("view-card");
      setAttributeIfPresent(card, "role", "button");
      // Not a tab stop: the dock's tab walk is the bar's own, and the switch itself
      // (plus Ctrl+5) is the keyboard's way here. The card stays click- and
      // programmatic-focus-reachable.
      setAttributeIfPresent(card, "tabindex", "-1");
      setAttributeIfPresent(card, "data-view", view);
      if (card.dataset) card.dataset.view = view;
      const cardLabel = VIEW_LABELS[view];
      card.textContent = cardLabel;
      setAttributeIfPresent(card, "title", `Show ${cardLabel}`);
      setAttributeIfPresent(card, "aria-label", `Show ${cardLabel}`);
      const pick = (): void => {
        deps.choose(view);
        cardsShown = false;
        sync();
      };
      listen(card, "click", () => pick());
      listen(card, "keydown", (event: KeyboardEvent) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          pick();
        }
      });
      appendChild(container, card);
    }
    container.hidden = !cardsShown;
  }

  function toggleCards(): void {
    cardsShown = !cardsShown;
    sync();
  }

  function hideCards(): void {
    if (!cardsShown) return;
    cardsShown = false;
    sync();
  }

  function unmount(): void {
    if (listening) listening.unmount();
    listening = null;
  }

  return { sync, cycle, toggleCards, hideCards, unmount };
}

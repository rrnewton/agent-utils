// The three-way view selector for the main screen's switch (`#view-switch`): the button names the
// view being shown, a press cycles Voice -> Channel -> Documents -> Voice, and a horizontal row
// of three cells in `#view-cards` (Voice | Channel | Documents, `#1078 bottom-bar-switcher`)
// shows the rotation as tabs while the reader taps through, the landed view's cell highlighted.
// Each cell also chooses its view directly. The row takes itself away shortly after the last
// press (see `VIEW_CARDS_SETTLE_MS`). Construction performs no effects: no element is looked up,
// no listener is added and no timer is started until `sync`, `cycle` or `toggleCards` is called.

import { createListening } from "../dom/index.ts";
import type { Listening } from "../dom/index.ts";

/** The three views the switch rotates through. */
export type DocViewName = "voice" | "discord" | "docs";

/** The views in rotation order: Voice, then Channel, then Documents, then Voice again. */
export const VIEW_ORDER: readonly DocViewName[] = Object.freeze(["voice", "discord", "docs"]);

/** The word shown for each view: on the switch for the current view, and on a cell for each. */
export const VIEW_LABELS: Readonly<Record<DocViewName, string>> = Object.freeze({
  voice: "Voice",
  discord: "Channel",
  docs: "Documents",
});

/**
 * How long the tab row stays up after the last press, in ms: the delay the stacked cards had,
 * kept. It used to be a stylesheet animation from the first show, which neither restarted on a
 * further press (so the row could vanish mid-tap-through) nor told the switch's `aria-expanded`
 * that the row had gone; the selector times it from the last press instead, and hiding says so.
 */
export const VIEW_CARDS_SETTLE_MS = 600;

/** The view after `current` in the rotation: voice -> discord -> docs -> voice. */
export function nextView(current: DocViewName): DocViewName {
  const index = VIEW_ORDER.indexOf(current);
  return VIEW_ORDER[(index + 1) % VIEW_ORDER.length] ?? "voice";
}

/**
 * The two views that are not `current`, in the order they come next when rotating: for Voice,
 * Channel then Documents; and so on around the rotation. Kept for callers that name what comes
 * next; the tab row itself shows all three views in `VIEW_ORDER`.
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
  /** Arm the settle timer; the page's own timers where given, the platform's otherwise. */
  setTimer?(run: () => void, ms: number): unknown;
  /** Forget a settle timer armed by `setTimer`. */
  clearTimer?(handle: unknown): void;
  /** The settle delay in ms; `VIEW_CARDS_SETTLE_MS` where not given. */
  settleMs?: number;
}

/** The view selector, once created. */
export interface ViewSelector {
  /**
   * Draw the selector for `current()`: the switch's word, title and accessible state, and the
   * three tab cells with the current view's marked. The row is hidden unless it has been shown
   * (see `cycle` and `toggleCards`). The first call also wires the cells' clicks.
   */
  sync(): void;
  /** Choose the next view in the rotation, show the tab row, and restart its settle. */
  cycle(): void;
  /** Show the tab row if it is hidden (restarting its settle), and hide it if it is shown. */
  toggleCards(): void;
  /** Hide the tab row and forget its settle (the page calls this once it has been seen). */
  hideCards(): void;
  /** Take down every listener and timer the selector added. */
  unmount(): void;
}

function setAttributeIfPresent(element: HTMLElement, name: string, value: string): void {
  if (typeof element.setAttribute === "function") element.setAttribute(name, value);
}

function removeAttributeIfPresent(element: HTMLElement, name: string): void {
  if (typeof element.removeAttribute === "function") element.removeAttribute(name);
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
  let settle: unknown = null;

  const setTimer = deps.setTimer ?? ((run: () => void, ms: number): unknown => setTimeout(run, ms));
  const clearTimer = deps.clearTimer ?? ((handle: unknown): void => {
    clearTimeout(handle as ReturnType<typeof setTimeout>);
  });
  const settleMs = deps.settleMs ?? VIEW_CARDS_SETTLE_MS;

  const ensureListening = (): Listening => {
    if (!listening) listening = createListening();
    return listening;
  };

  /** Forget the settle, if one is armed. */
  function clearSettle(): void {
    if (settle === null) return;
    clearTimer(settle);
    settle = null;
  }

  /** (Re)arm the settle from now: the row goes `settleMs` after the last press that showed it. */
  function armSettle(): void {
    clearSettle();
    settle = setTimer(() => {
      settle = null;
      hideCards();
    }, settleMs);
  }

  function cycle(): void {
    deps.choose(nextView(deps.current()));
    cardsShown = true;
    sync();
    armSettle();
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
    // too would cycle twice per press, so only the cells are wired here.
    const { listen } = ensureListening();

    const container = deps.byId("view-cards");
    container.hidden = !cardsShown;
    clearChildren(container);

    const doc = button.ownerDocument ?? (typeof document !== "undefined" ? document : null);
    if (!doc) return;

    for (const view of VIEW_ORDER) {
      // A cell, not a <button>: the bar's own buttons are counted and laid out as the
      // dock's controls, and these three are a pop-up over it, not members of it.
      const card = doc.createElement("div");
      card.className = "view-card";
      if (typeof card.classList?.add === "function") card.classList.add("view-card");
      setAttributeIfPresent(card, "role", "button");
      // Not a tab stop: the dock's tab walk is the bar's own, and the switch itself
      // (plus Ctrl+5) is the keyboard's way here. The cell stays click- and
      // programmatic-focus-reachable.
      setAttributeIfPresent(card, "tabindex", "-1");
      setAttributeIfPresent(card, "data-view", view);
      if (card.dataset) card.dataset.view = view;
      const cardLabel = VIEW_LABELS[view];
      card.textContent = cardLabel;
      const isCurrent = view === current;
      if (isCurrent) {
        if (typeof card.classList?.add === "function") card.classList.add("view-card-current");
        else card.className = `${card.className} view-card-current`.trim();
        setAttributeIfPresent(card, "aria-current", "true");
        setAttributeIfPresent(card, "title", `Showing ${cardLabel}`);
        setAttributeIfPresent(card, "aria-label", `Showing ${cardLabel}`);
      } else {
        removeAttributeIfPresent(card, "aria-current");
        setAttributeIfPresent(card, "title", `Show ${cardLabel}`);
        setAttributeIfPresent(card, "aria-label", `Show ${cardLabel}`);
      }
      const pick = (): void => {
        // The current cell only dismisses: choosing the view already up is no choice.
        if (!isCurrent) deps.choose(view);
        hideCards();
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
    if (cardsShown) {
      hideCards();
      return;
    }
    cardsShown = true;
    sync();
    armSettle();
  }

  function hideCards(): void {
    clearSettle();
    if (!cardsShown) return;
    cardsShown = false;
    sync();
  }

  function unmount(): void {
    clearSettle();
    if (listening) listening.unmount();
    listening = null;
  }

  return { sync, cycle, toggleCards, hideCards, unmount };
}

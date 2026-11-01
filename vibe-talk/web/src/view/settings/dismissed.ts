// Settings' Recently dismissed, and the pop-out over it (`#348 settings-dynamic-lists`, `#351
// settings-polish`; `#624 voice-page-modularity`, moved from the legacy script; plan step 16,
// view/settings).
//
// THE LIST. A data panel rather than a column of cards with a Put back on each: a row is one button, its
// two lines the place and age and the text, cut to one line by the stylesheet (the whole text is still in
// the row, for find-in-page), and it opens the pop-out where the whole message is, with Copy and Put back.
// A screen reader is told the row's name, `dismissedRowLabel`, rather than everything in it.
//
// THE POP-OUT. One message, whole: where it was going and when it was dismissed, its text as typed, Copy,
// Put back and Close. It is a menu to the keys and to the back gesture (the page's `openMenus`): Escape
// closes it first, and so does Back, before Settings. Leaving Settings closes it.
//
// `#351 settings-polish`. A MODAL. It dims the screen around it, and the dim used to be only a shadow:
// Tab went on past Close to the controls behind it, and a tap on the dim pressed whatever was under it.
// So while it is up everything else on the page is inert (`inertAround`), which no tap, click or Tab
// reaches and a screen reader does not list, and Tab and Shift+Tab go round its own four stops
// (`trapTab`). It is marked `aria-modal` to say so. Close, Escape and Back are how it goes.
//
// And everything it makes inert is under the dim. The dim is the pop-out's shadow, which Settings'
// scrolling box clips (`dimBoxOf`), so the top bar with its Back stood bright above it, and a tap on
// that Back did nothing while it looked live. What is inert beyond that box and laid out beside it,
// the top bar and the dock, is now dimmed by the same depth where it stands (`data-under-dim`). The
// status pill is not: it floats over the box from outside it, under the pop-out, so the shadow dims it
// already (`floatsOverDimBox`). One dim over the whole screen, nothing dimmed twice, and the pop-out
// drawn over everything, its buttons included when it stands at the foot of the screen where the pill
// is.
//
// What a send is, putting one back in the outbox, and the copy kept on the device are the page's and
// web/src/core/outbox's; the words are web/src/present/settings's.

import { unsentText } from "../../core/outbox/index.ts";
import type { DismissedSends } from "../../core/outbox/index.ts";
import { dismissedListWords, dismissedRestoreLabel, dismissedRowLabel, dismissedSendMeta } from "../../present/settings/index.ts";
import type { DismissedEntry } from "../../present/settings/index.ts";

/** The pop-out's Tab stops, in order: the message, Copy, Put back and Close. */
const DISMISSED_POPOUT_STOPS: readonly string[] = Object.freeze(["dismissed-send-popout-text", "dismissed-send-copy", "dismissed-send-restore", "dismissed-send-close"]);

/** The values of `overflow` that clip what overflows a box, a shadow drawn inside it included. */
const CLIPPING_OVERFLOW: ReadonlySet<string> = new Set(["auto", "scroll", "hidden", "clip"]);

/**
 * The box `node`'s dim is drawn in: the nearest element around it that clips its overflow, or the
 * body. The dim is a shadow, and a shadow is overflow like any other, so on Settings it ends at the
 * edges of the scrolling #screen-settings and is not drawn over the top bar or the dock.
 * `#351 settings-polish`.
 */
function dimBoxOf(node: Element): Element {
  if (typeof window.getComputedStyle !== "function") return document.body;
  for (let at = node.parentNode as Element | null; at && at !== document.body; at = at.parentNode as Element | null) {
    const style = window.getComputedStyle(at);
    if (CLIPPING_OVERFLOW.has(style.overflowX) || CLIPPING_OVERFLOW.has(style.overflowY)) return at;
  }
  return document.body;
}

/**
 * Whether `node`, beside the box the pop-out's dim is drawn in or beside an element around that box,
 * floats over the box rather than standing next to it: taken out of the flow (`position: absolute`),
 * as the status pill is over the foot of the screens (`#63 status-line-placement`). What stays in the
 * flow there, the top bar, the error line, the dock, is laid out beside the box, where the shadow
 * cannot reach. What floats over it is drawn under the pop-out, whose z-index is above the frame's
 * overlays (web/voice.css), and so under its shadow already. `#351 settings-polish`.
 */
function floatsOverDimBox(node: Element): boolean {
  return typeof window.getComputedStyle === "function" && window.getComputedStyle(node).position === "absolute";
}

/**
 * Make everything on the page but `node` inert: each element beside it, and beside every element
 * around it, up to the body. Returns what it made inert, for the close to give back; an element
 * already inert is left as it was, to whatever made it so.
 *
 * `#351 settings-polish`. What it makes inert beyond the box `node`'s dim is drawn in (`dimBoxOf`),
 * laid out beside that box, is marked `data-under-dim` too, and web/voice.css dims it by the shadow's
 * own depth: a control nothing can reach is never left looking as if it could be pressed. What floats
 * over the box (`floatsOverDimBox`) is left to the shadow, which dims it already.
 */
function inertAround(node: Element): Element[] {
  const made: Element[] = [];
  const box = dimBoxOf(node);
  let beyond = false;
  for (let at = node; at !== document.body && at.parentNode; at = at.parentNode as Element) {
    // From the dim's own box outwards, what stands beside is out of the shadow's reach, unless it
    // floats over the box, under the pop-out and its shadow.
    if (at === box) beyond = true;
    for (const beside of (at.parentNode as Element).children) {
      if (beside === at || beside.hasAttribute("inert")) continue;
      beside.setAttribute("inert", "");
      if (beyond && !floatsOverDimBox(beside)) beside.setAttribute("data-under-dim", "");
      made.push(beside);
    }
  }
  return made;
}

/** What the list and its pop-out ask of the page. */
export interface DismissedSendsViewDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** The sends dismissed from the outbox, and the device's copy of them (`dismissedSends`). */
  store(): DismissedSends<DismissedEntry>;
  /** What the page calls a channel, by id (`channelLabel`). */
  channelLabel(id: string): string;
  /** An element with a class and text (`styled`). */
  styled(tag: string, className: string, text: string): HTMLElement;
  /** Copy text to the clipboard, saying whether it was (`writeClipboardText`). */
  writeClipboard(text: string): Promise<boolean>;
  /** What the page says when the browser would not copy (`COPY_REFUSED`). */
  copyRefused(): string;
  /** Put a dismissed send back in the outbox (`putBackDismissedSend`). */
  putBack(id: string): void;
  /** The back gesture's entries follow what is open (`scheduleBackSync`). */
  scheduleBackSync(): void;
}

/** The list's and the pop-out's named commands. */
export interface DismissedSendsView {
  /** Settings' Recently dismissed: each send's place, its age and the first line of its text. */
  render(): void;
  /** Open the pop-out on the dismissed send `id`, from the row `opener`. */
  open(id: string, opener: HTMLElement): void;
  /** Close the pop-out, giving the focus back to the row that opened it when `refocus` and it is still there. */
  close(refocus?: boolean): boolean;
  /** Tab and Shift+Tab while the pop-out is up: round its own stops. */
  trapTab(event: KeyboardEvent): void;
  /** The pop-out's Copy: the whole message to the clipboard. */
  copy(): Promise<void>;
  /** The pop-out's Put back: the message back in the outbox, and the focus on the row in its place. */
  restore(): void;
}

/** No pop-out open. Made where the page keeps its outbox's dismissed sends; it wires nothing itself. */
export function mountDismissedSends(deps: DismissedSendsViewDependencies): DismissedSendsView {
  /**
   * `#348 settings-dynamic-lists`. The dismissed send the pop-out over Recently dismissed shows, and the
   * row that opened it, which takes the focus back. `inert` is what the pop-out made inert behind it
   * while it is up, and dimmed where its shadow does not reach (`#351 settings-polish`, `inertAround`,
   * `dimBoxOf`).
   */
  let dismissedPopout: { id: string; opener: HTMLElement | null; inert: Element[] } | null = null;

  function open(id: string, opener: HTMLElement): void {
    const held = deps.store().find(id);
    if (!held) return;
    // One at a time: what the one before made inert is given back before this one makes its own.
    close(false);
    deps.lookup("dismissed-send-popout-meta").textContent = dismissedSendMeta(held, deps.channelLabel);
    deps.lookup("dismissed-send-popout-text").textContent = unsentText(held.entry);
    deps.lookup("dismissed-send-restore").setAttribute("aria-label", dismissedRestoreLabel(held.entry));
    deps.lookup("dismissed-send-popout-state").textContent = "";
    const popout = deps.lookup("dismissed-send-popout");
    popout.hidden = false;
    // What it makes inert beyond the box its shadow is drawn in, and beside that box, is dimmed where it is.
    dismissedPopout = { id, opener, inert: inertAround(popout) };
    // Laid over its group, which may stand partly below the screen's edge: brought on screen whole.
    if (typeof popout.scrollIntoView === "function") popout.scrollIntoView({ block: "nearest" });
    deps.lookup("dismissed-send-copy").focus({ preventScroll: true });
    deps.scheduleBackSync();
  }

  function close(refocus = true): boolean {
    if (!dismissedPopout) return false;
    const { opener, inert } = dismissedPopout;
    dismissedPopout = null;
    // The page behind it back first: an inert element cannot take the focus.
    for (const node of inert) {
      node.removeAttribute("inert");
      node.removeAttribute("data-under-dim");
    }
    deps.lookup("dismissed-send-popout").hidden = true;
    if (refocus && opener && opener.parentNode && typeof opener.focus === "function") opener.focus({ preventScroll: true });
    deps.scheduleBackSync();
    return true;
  }

  /**
   * After Put back: the focus on the row that took the place of the one put back (`at` in the list),
   * else on the list, else on the line that says it is empty. `#351 settings-polish`. The row that
   * opened the pop-out went with its message, and the focus with it, to nowhere.
   */
  function focusDismissedAt(at: number): void {
    const list = deps.lookup("dismissed-sends");
    const item = at >= 0 ? list.children[at] : undefined;
    const row = item ? item.firstChild as HTMLElement | null : null;
    (row || (list.hidden ? deps.lookup("dismissed-sends-state") : list)).focus();
  }

  return {
    render() {
      const store = deps.store();
      store.prune();
      const items = store.list().map((held) => {
        const entry = held.entry;
        const item = document.createElement("li");
        item.className = "dismissed-send";
        const opener = document.createElement("button");
        opener.className = "dismissed-send-open";
        opener.setAttribute("type", "button");
        opener.setAttribute("aria-haspopup", "dialog");
        opener.setAttribute("aria-controls", "dismissed-send-popout");
        opener.setAttribute("data-outgoing-id", String(entry.id));
        opener.setAttribute("aria-label", dismissedRowLabel(held, deps.channelLabel));
        opener.addEventListener("click", () => open(entry.id as string, opener));
        opener.append(
          deps.styled("span", "dismissed-send-meta", dismissedSendMeta(held, deps.channelLabel)),
          deps.styled("span", "dismissed-send-text", unsentText(entry))
        );
        item.append(opener);
        return item;
      });
      deps.lookup("dismissed-sends").replaceChildren(...items);
      deps.lookup("dismissed-sends").hidden = items.length === 0;
      deps.lookup("forget-dismissed-sends").hidden = items.length === 0;
      deps.lookup("dismissed-sends-state").textContent = dismissedListWords(items.length);
      // The pop-out is about one of them: gone from the list, it goes too.
      if (dismissedPopout && !store.find(dismissedPopout.id)) close(false);
    },
    open,
    close,
    /**
     * From the last to the first and the first to the last, and from anywhere outside them (the page,
     * after a tap on the dim) to its first or last. A key the page has already taken, or one held with
     * Ctrl, Alt or Cmd, is left alone.
     */
    trapTab(event) {
      if (!dismissedPopout || !event || event.key !== "Tab" || event.defaultPrevented ||
          event.ctrlKey || event.altKey || event.metaKey) return;
      const stops = DISMISSED_POPOUT_STOPS.map((id) => deps.lookup(id));
      const at = stops.indexOf(document.activeElement as HTMLElement);
      if (at !== -1 && at !== (event.shiftKey ? 0 : stops.length - 1)) return;
      event.preventDefault();
      (stops[event.shiftKey ? stops.length - 1 : 0] as HTMLElement).focus();
    },
    async copy() {
      const held = dismissedPopout && deps.store().find(dismissedPopout.id);
      if (!held) return;
      const copied = await deps.writeClipboard(unsentText(held.entry));
      deps.lookup("dismissed-send-popout-state").textContent = copied ? "Copied the whole message." : deps.copyRefused();
    },
    restore() {
      if (!dismissedPopout) return;
      const id = dismissedPopout.id;
      const at = deps.store().list().findIndex((held) => held.entry.id === id);
      close(false);
      deps.putBack(id);
      focusDismissedAt(at);
    },
  };
}

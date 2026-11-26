// The on-screen keyboard (`#429 keyboard-hides-bar`, `#575 mobile-keyboard-compose`, `#610`; `#624
// voice-page-modularity`, moved from the legacy script into view/composer): deciding whether a phone's
// keyboard is up from the visual viewport and the field in focus, telling the stylesheet (`data-keyboard`,
// `--keyboard-inset`), bringing the composer's foot to the keyboard's top in the jump's own moment, and the
// readout of the numbers behind it for a screenshot. The listeners that hear the viewport, the focus and the
// reader, and the Settings switch for the readout, are the page's; why each rule is as it is is told where
// the legacy script opens the section.

import type { ClockPort } from "../../ports/index.ts";
import { isTextField, nodeWithin } from "../dom/index.ts";
import type { GeometryName } from "../dom/index.ts";
import { COMPOSER_FIELDS } from "./keys.ts";

/** The least height, in CSS pixels, an on-screen keyboard takes: more than a browser's toolbar. */
const KEYBOARD_MIN_PX = 150;
/** ...and the least share of the tallest viewport it takes, for a tall screen's taller toolbars. */
const KEYBOARD_MIN_SHARE = 0.2;

/**
 * `#610 mobile-keyboard-compose`. How long the jump to the keyboard's top is made again as the browser goes on
 * moving the viewport for the keyboard, in milliseconds: Safari animates its keyboard up and then pans
 * the visual viewport over the focused field, which can come after the resize the jump was made at. A
 * touch, a press or a wheel ends it at once (`readerMoved`).
 */
const KEYBOARD_SETTLE_MS = 1200;

/** The channel's and the right column's box: each with its composer (its Send and state line kept for the readout's names). */
const KEYBOARD_FIT_PARTS: Readonly<Record<string, readonly [string, string, string]>> = {
  "channel-compose-text": ["channel-composer", "channel-send", "channel-compose-state"],
  "side-compose-text": ["side-composer", "side-send", "side-compose-state"],
};

/**
 * `ios-keyboard-send`. Each sending box's own Send, the one its row already holds: the Send at the
 * keyboard's top presses this very button, so the two can never become two opinions about sending.
 * The call's box is not here: its Send stands in the dock, which stays over the keyboard in the
 * call's view (`#241 voice-composer`), so it is never the one the keyboard covers.
 */
const KEYBOARD_SEND_PARTS: Readonly<Record<string, string>> = {
  "channel-compose-text": "channel-send",
  "side-compose-text": "side-send",
  "reply-text": "reply-send",
  "quick-reply-text": "quick-reply-send",
  "discuss-followup": "discuss-send",
};

/** The room the Send at the keyboard's top keeps above the keys, in CSS pixels. */
const KEYBOARD_SEND_GAP_PX = 8;

/** The visual viewport, as far as the keyboard reads it. */
interface Viewport {
  readonly height: number;
  readonly scale?: number;
  readonly offsetTop?: number;
  readonly pageTop?: number;
}

/** What the keyboard asks of the page. */
export interface OnScreenKeyboardDependencies {
  readonly clock: ClockPort;
  /** The page's element by id, in the left column (`leftEl`). */
  lookup(id: string): HTMLElement;
  /** The page's element by id, in the column being worked on (`el`). */
  el(id: string): HTMLElement;
  /** The browser's visual viewport, or null where it has none. */
  viewport(): Viewport | null;
  /** The layout viewport's height (`innerHeight`), 0 where none is said. */
  innerHeight(): number;
  /** The document's own scroll (`scrollY`). */
  scrollY(): number;
  /** The root element. */
  root(): HTMLElement;
  /** The element with the focus. */
  activeElement(): Element | null;
  /** What `navigator.virtualKeyboard` says of its rectangle, where the browser has it. */
  virtualKeyboard(): { readonly boundingRect?: { readonly height: number } } | undefined;
  /** A node's computed style, or null where the browser has no `getComputedStyle`. */
  computedStyle(node: Element): CSSStyleDeclaration | null;
  /** Close the pace popover, whose button the keyboard takes away (`closeSpeedPopover`). */
  closeSpeedPopover(): void;
  /** Move `area` by a measured delta through the scroll owner (`#1251`). */
  nudge(area: HTMLElement, delta: number): void;
  /** Take `area` to its foot through the scroll owner (`#1251`). */
  toFoot(area: HTMLElement): void;
  /**
   * `ios-keyboard-send`. The Send that stands at the keyboard's top (`#keyboard-send`), where the
   * page has it; absent, no second Send is stood. Optional: a page built without it still fits.
   */
  cornerSend?(): HTMLElement | null | undefined;
  /** `ios-keyboard-send`. `field`'s own Send (`KEYBOARD_SEND_PARTS`), or null where it has none. */
  normalSend?(field: Element): HTMLElement | null | undefined;
  /**
   * `#2277`: hand the keyboard's room to the boxes that keep it (`createGeometry`'s `write`), never
   * to the root every row inherits from. Absent (a double), the root is written as before.
   */
  writeGeometry?(name: GeometryName, value: string): void;
}

/** The keyboard's commands. */
export interface OnScreenKeyboard {
  /** Decide again whether the on-screen keyboard is up, tell the stylesheet when that changes, and fit. */
  noteKeyboard(): void;
  /** The reader touching, pressing or turning a wheel: the jump's moment is over. */
  noteKeyboardReader(): void;
  /** A scroll the browser made in the jump's moment: the jump is made again. */
  settleOnScroll(): void;
  /**
   * Text was put in the fitted box while its reader rests at the foot (`#1144
   * scrollback-typing-jump`): once the browser has had its own say (it reveals the caret, which
   * shows the caret's line and no more), the jump is made whole again, so the whole composer
   * stands on the keyboard's top. The caller has judged the reader at the foot; a reader up the
   * list is the typing guard's, never this one's.
   */
  noteComposerInput(field: Element): void;
  /**
   * The room, in px, the keyboard keeps under its fitted composer in `area` (`#1144
   * scrollback-typing-jump`): counted in the list's height but no part of its rows, so the foot
   * the page measures can leave it out. 0 for any other list, and while no box is fitted.
   */
  roomIn(area: HTMLElement): number;
  /**
   * `#2277 vibe-talk-key-scroll-freezes-11-26`: where the fitted `composer`'s own foot is (the bottom
   * of what it shows, its Send row included, without the keyboard's room under it), the one foot the
   * keepers bring to the keyboard's top; null while no box is fitted, or for another box.
   */
  fittedFoot(composer: Element): number | null;
  /** Draw the readout where `on`, keep it redrawn while it is, and set its switch. */
  showKeyboardDebug(on: boolean): void;
  /** Draw the readout, where it is on. */
  renderKeyboardDebug(): void;
  /**
   * `ios-keyboard-send`. Press the focused box's own Send from the Send at the keyboard's top:
   * true where it pressed one. False while that Send already stands in the visible part, where
   * the corner Send is down and its press would be a second send of the same words.
   */
  pressCornerSend(): boolean;
}

/** No keyboard yet. */
export function createOnScreenKeyboard(deps: OnScreenKeyboardDependencies): OnScreenKeyboard {
  /**
   * The keyboard as last decided; the tallest visible height seen at each width the window has had,
   * which is that width's screen with no keyboard; the width last measured; and, across a turn of the
   * phone with the keyboard up, the height the new width was first measured at (`turnedAt`), null otherwise.
   *
   * `shown`: whether the page lays itself out for it (`data-keyboard`), which it does not for the pace box.
   *
   * `fitted`: the composer's box the page last brought to the keyboard's top (`fitComposerToKeyboard`), and
   * `fittedHeight`, the visual viewport's height then: the jump is made as the keyboard comes up, as the
   * focus moves to another box, and as the keyboard itself changes size, and at no other time (`#610`).
   * `settling`: the jump's own moment, while the browser is still moving the viewport for the keyboard
   * (Safari pans after its resize), in which it is made again as the viewport moves, and `generation`,
   * counting jumps, so a moment's end is only its own; `readerMoved`, a touch, a press or a wheel since
   * the jump, which ends that moment at once. `inset`, the room a composer keeps under itself for the
   * keyboard (`renderKeyboardInset`); `lastFit`, what the last jump measured, for the readout;
   * `fittedArea`, the list the fitted composer stands in, whose height that room is part of,
   * and `fittedRoom`, all of it past the rows' foot (the inset and any row parked under the box)
   * (`roomIn`); `inputDue`, a whole-composer completion already waiting for its task.
   */
  const onScreenKeyboard: {
    up: boolean; shown: boolean; tallest: Map<number, number>; width: number; turnedAt: number | null;
    fitted: Element | null; fittedHeight: number; settling: boolean; generation: number; readerMoved: boolean;
    inset: number; lastFit: { target: number; moved: number; field: string } | null;
    fittedArea: HTMLElement | null; fittedRoom: number; inputDue: boolean;
    cornerField: Element | null; cornerShown: boolean;
  } = { up: false, shown: false, tallest: new Map(), width: -1, turnedAt: null, fitted: null,
    fittedHeight: 0, settling: false, generation: 0, readerMoved: false, inset: 0, lastFit: null,
    fittedArea: null, fittedRoom: 0, inputDue: false, cornerField: null, cornerShown: false };

  /** Whether the readout is drawn; see `showKeyboardDebug`. */
  let keyboardDebug = false;
  /** The readout's own redraw while it is drawn, for what moves with no event of the page's: its timer, or 0. */
  let keyboardDebugTimer: unknown = 0;

  function noteKeyboard(): void {
    const field = deps.activeElement();
    const typing = isTextField(field);
    const viewport = deps.viewport();
    const visible = viewport ? viewport.height * (viewport.scale || 1) : deps.innerHeight() || 0;
    const root = deps.root();
    const width = (root && root.clientWidth) || 0;
    const turned = width !== onScreenKeyboard.width && onScreenKeyboard.width !== -1;
    onScreenKeyboard.width = width;
    const measured = Math.max(visible, deps.innerHeight() || 0);
    const tallest = typing ? Math.max(onScreenKeyboard.tallest.get(width) || 0, measured) : measured;
    onScreenKeyboard.tallest.set(width, tallest);
    const least = (height: number): number => Math.max(KEYBOARD_MIN_PX, height * KEYBOARD_MIN_SHARE);
    if (!typing) onScreenKeyboard.turnedAt = null;
    else if (turned && onScreenKeyboard.up) onScreenKeyboard.turnedAt = visible;
    else if (onScreenKeyboard.turnedAt !== null && visible - onScreenKeyboard.turnedAt >= least(tallest)) {
      onScreenKeyboard.turnedAt = null;
    }
    const virtual = deps.virtualKeyboard();
    const reported = Boolean(virtual && virtual.boundingRect && virtual.boundingRect.height > 0);
    const shrunk = visible > 0 && tallest - visible >= least(tallest);
    const up = typing && (reported || shrunk || onScreenKeyboard.turnedAt !== null);
    // `#567 pace-stepper`. The pace box raises the keyboard too, but what is being used then is its
    // popover and the bar it stands on: neither goes, as they do for a message being written.
    const shown = up && field !== deps.el("read-speed-box");
    if (up !== onScreenKeyboard.up || shown !== onScreenKeyboard.shown) {
      onScreenKeyboard.up = up;
      onScreenKeyboard.shown = shown;
      root.setAttribute("data-keyboard", shown ? "open" : "closed");
      if (shown && !deps.el("speed-popover").hidden) deps.closeSpeedPopover();
    }
    // `#575 mobile-keyboard-compose`, `#610`. Once the stylesheet has the keyboard, so the box is measured as drawn
    // for it. The jump is made as the keyboard comes up or the focus moves to another box; as the keyboard
    // itself changes size (its suggestion strip, its emoji keys), unless the reader has moved since; and
    // again as the viewport moves in the jump's own moment (`settling`). Never otherwise: a pan or a scroll
    // of the reader's own is theirs.
    const fit = shown && field && KEYBOARD_FIT_PARTS[field.id] ? field : null;
    const height = viewport ? Number(viewport.height) || 0 : deps.innerHeight() || 0;
    if (fit !== onScreenKeyboard.fitted) {
      // `#1251 scroll-single-owner`: as the keyboard goes down the list regains the fitted
      // room; a reader who was at its foot is taken back to it by the owner, as native
      // scroll anchoring used to do silently. Noted before the room is cleared.
      const fittedArea = onScreenKeyboard.fittedArea;
      const wasAtFoot = fit === null && fittedArea !== null && !onScreenKeyboard.readerMoved;
      onScreenKeyboard.fitted = fit;
      onScreenKeyboard.settling = false;
      if (fit) startKeyboardFit(fit, height);
      else {
        onScreenKeyboard.fittedArea = null;
        onScreenKeyboard.fittedRoom = 0;
        renderKeyboardInset(0);
        if (wasAtFoot && fittedArea !== null) deps.toFoot(fittedArea);
      }
    } else if (fit && Math.abs(height - onScreenKeyboard.fittedHeight) >= 1) {
      // The room follows the keyboard's size whatever the reader is doing: without it the box could sit
      // under a grown keyboard with no scroll left to reach it.
      onScreenKeyboard.fittedHeight = height;
      renderKeyboardInset(keyboardInset(fit));
      if (!onScreenKeyboard.readerMoved) startKeyboardFit(fit, height);
    } else if (fit && onScreenKeyboard.settling) {
      settleKeyboardFit(fit);
    }
    renderCornerSend(shown, field, viewport);
    renderKeyboardDebug();
  }

  /**
   * `ios-keyboard-send`. Stand a Send where the reader can always reach it: in the corner above
   * the keyboard, like the new-line key (`#575`), for whichever sending box holds the focus —
   * but only while no part of that box's own Send stands in the visual viewport, under the
   * keyboard or above its pan, so the two never stand together (`#1680
   * inline-reply-keyboard-clutter`: the reply in the stream is never fitted, so its row can
   * straddle the keyboard's top, half on screen; a fully-inside test stood the corner Send
   * over that visible half, and the owner saw two Sends overlapping). Its foot is the
   * keyboard's top, counted from the visual viewport (`offsetTop + height`, `#610`) up from
   * the layout viewport's foot, and said again on every measure, which the viewport's own
   * resize and scroll already ask for: as Safari pans and the keyboard changes size the Send
   * follows it. No keyboard, no sending box, or any part of the row's Send on screen, and it
   * stands down: the focused field's own row is the one Send.
   */
  function renderCornerSend(shown: boolean, field: Element | null, viewport: Viewport | null): void {
    const button = typeof deps.cornerSend === "function" ? deps.cornerSend() : null;
    const cornerField = shown && field && COMPOSER_FIELDS.includes(field.id) && KEYBOARD_SEND_PARTS[field.id]
      ? field : null;
    onScreenKeyboard.cornerField = cornerField;
    onScreenKeyboard.cornerShown = false;
    if (!button) return;
    let show = false;
    if (cornerField) {
      const normal = typeof deps.normalSend === "function" ? deps.normalSend(cornerField) : null;
      if (!normal) show = true;
      else if (typeof normal.getBoundingClientRect === "function") {
        const rect = normal.getBoundingClientRect();
        const keysTop = viewport ? (Number(viewport.offsetTop) || 0) + (Number(viewport.height) || 0)
          : deps.innerHeight() || 0;
        const viewTop = viewport ? Number(viewport.offsetTop) || 0 : 0;
        const onScreen = rect.bottom > rect.top && rect.bottom > viewTop && rect.top < keysTop;
        show = !onScreen;
      } else show = true;
      if (show) {
        const keysTop = viewport ? (Number(viewport.offsetTop) || 0) + (Number(viewport.height) || 0)
          : deps.innerHeight() || 0;
        let foot = Math.max(0, Math.floor((deps.innerHeight() || 0) - keysTop)) + KEYBOARD_SEND_GAP_PX;
        // `#1813 vibe-talk-overlays-center-on-channel-pane`: while Discuss is docked under
        // the list on a phone, the channel pane's foot is the sheet's top, well above the
        // keyboard's top: the corner Send stands at the pane's foot, never over the sheet.
        const area = ["scroll-area", "side-scroll"].map((id) => deps.lookup(id))
          .find((list) => list && nodeWithin(list, cornerField));
        if (area && typeof area.getBoundingClientRect === "function") {
          const paneFoot = area.getBoundingClientRect().bottom + (viewport ? Number(viewport.offsetTop) || 0 : 0);
          if (paneFoot < keysTop) {
            foot = Math.max(foot, Math.max(0, Math.floor((deps.innerHeight() || 0) - paneFoot)) + KEYBOARD_SEND_GAP_PX);
          }
        }
        const style = (button as HTMLElement).style as { bottom?: string } | undefined;
        if (style) style.bottom = `${foot}px`;
      }
    }
    onScreenKeyboard.cornerShown = show;
    (button as HTMLElement & { hidden: boolean }).hidden = !show;
  }

  /** Press the corner field's own Send: the corner Send's whole act (`renderCornerSend`). */
  function pressCornerSend(): boolean {
    if (!onScreenKeyboard.cornerShown || !onScreenKeyboard.cornerField) return false;
    const normal = typeof deps.normalSend === "function" ? deps.normalSend(onScreenKeyboard.cornerField) : null;
    if (!normal || (normal as HTMLButtonElement).disabled) return false;
    if (typeof (normal as HTMLElement).click !== "function") return false;
    (normal as HTMLElement).click();
    return true;
  }

  /**
   * `#610 mobile-keyboard-compose`. Jump to the keyboard's top for `field`, and go on doing so as the viewport moves
   * until the keyboard has settled (`KEYBOARD_SETTLE_MS`) or the reader moves. `height`: the visual viewport's
   * height the jump is made at.
   */
  function startKeyboardFit(field: Element, height: number): void {
    onScreenKeyboard.fittedHeight = height;
    onScreenKeyboard.readerMoved = false;
    onScreenKeyboard.settling = true;
    const generation = ++onScreenKeyboard.generation;
    onScreenKeyboard.lastFit = null;
    settleKeyboardFit(field);
    deps.clock.setTimeout(() => {
      if (onScreenKeyboard.generation === generation) onScreenKeyboard.settling = false;
    }, KEYBOARD_SETTLE_MS);
  }

  /**
   * `#610 mobile-keyboard-compose`. The jump itself: the room for the keyboard as it now stands, then the composer's
   * foot, its Send row included, brought to the keyboard's top (`#2277`; `#1135 raw-message-page` parked the row
   * under the keys).
   */
  function settleKeyboardFit(field: Element): void {
    renderKeyboardInset(keyboardInset(field));
    fitComposerToKeyboard(field);
  }

  function noteKeyboardReader(): void {
    if (!onScreenKeyboard.fitted) return;
    onScreenKeyboard.readerMoved = true;
    onScreenKeyboard.settling = false;
  }

  /**
   * `#575 mobile-keyboard-compose`, `#610`. The room, in CSS pixels, the composer `field` is written in keeps under
   * itself while the keyboard is up, so its box can be brought to the keyboard's top: as much of the list
   * it is in as the keyboard covers, from the visual viewport's foot to the list's own foot. Not from
   * `innerHeight`, which a browser may or may not shrink for the keyboard; less Safari's pan, so at the
   * list's very end the last thing in the composer, its Send row, stands on the keyboard's top (`#2277`);
   * measured only at the jump and as the keyboard changes size.
   */
  function keyboardInset(field: Element): number {
    const viewport = deps.viewport();
    const where = composerFit(field);
    if (!viewport || !where) return 0;
    const keysTop = (Number(viewport.offsetTop) || 0) + (Number(viewport.height) || 0);
    return Math.max(0, Math.floor(where.area.getBoundingClientRect().bottom - keysTop));
  }

  /** Tell the stylesheet what the keyboard covers (`--keyboard-inset`), which a composer keeps as room under itself. */
  function renderKeyboardInset(inset: number): void {
    if (inset === onScreenKeyboard.inset) return;
    onScreenKeyboard.inset = inset;
    if (deps.writeGeometry) deps.writeGeometry("--keyboard-inset", `${inset}px`);
    else deps.root().style.setProperty("--keyboard-inset", `${inset}px`);
  }

  /**
   * `#2277`: the bottom of what `composer` shows, its Send row included: its box's bottom less its
   * padding under that, which is the keyboard's room while the keyboard is up (`--keyboard-inset`).
   * One foot for the jump (`fitComposerToKeyboard`) and for the keeper that holds the box while rows
   * change (`keepComposerInView`): with two (the field's foot here, the whole box's there) each row
   * change raised the list by the Send row and the next jump dropped it back, a 70 px bounce per change.
   */
  function composerFoot(composer: Element): number {
    const bottom = composer.getBoundingClientRect().bottom;
    const style = deps.computedStyle(composer);
    const under = style ? Number.parseFloat(style.paddingBottom) || 0 : 0;
    return bottom - under;
  }

  /**
   * The composer `field` is written in, the list it is in, and its foot, where the stylesheet lays it out
   * for the keyboard (`--keyboard-fit`, which it sets under a coarse pointer alone); null elsewhere, as on
   * a desk whose window is made shorter with the box focused. `foot` is the box itself, which the readout
   * names; the jump keeps the composer's own foot (`composerFoot`, `#2277`), the row under the box — Send
   * raw message, Cancel and Send — included. The list is the one the composer is in.
   */
  function composerFit(field: Element | null): { area: HTMLElement; foot: Element; composer: HTMLElement } | null {
    const parts = field && KEYBOARD_FIT_PARTS[field.id];
    if (!parts) return null;
    const composer = deps.lookup(parts[0]);
    const style = deps.computedStyle(composer);
    if (!style) return null;
    if (style.getPropertyValue("--keyboard-fit").trim() !== "on") return null;
    const area = ["scroll-area", "side-scroll"].map((id) => deps.lookup(id)).find((list) => nodeWithin(list, composer));
    if (!area) return null;
    return { area, foot: field as Element, composer };
  }

  /**
   * `#575 mobile-keyboard-compose`. Bring the foot of the composer `field` is written in to the keyboard's top, so
   * nothing of the list is wasted between the two and nothing below is left on screen. `#610`: the keyboard's
   * top is the visual viewport's foot, `offsetTop + height`. A JUMP, NOT A CAP: made as the keyboard comes up,
   * after which the reader scrolls as they like. A list too short to scroll stays where it is.
   */
  function fitComposerToKeyboard(field: Element): void {
    const where = composerFit(field);
    if (!where) return;
    const { area, composer } = where;
    onScreenKeyboard.fittedArea = area;
    // `#2277`: the foot brought to the keyboard's top is the whole composer's, its Send row
    // included (the owner keeps Send in sight), not the box's alone with the row parked under the
    // keys (`#1135`): the keeper that holds the composer while rows change keeps the same foot
    // (`fittedFoot`), so the two never move the list against each other.
    const footBottom = composerFoot(composer);
    const viewport = deps.viewport();
    const keysTop = viewport ? (Number(viewport.offsetTop) || 0) + (Number(viewport.height) || 0) : deps.innerHeight() || 0;
    const target = Math.min(area.getBoundingClientRect().bottom, keysTop);
    const off = footBottom - target;
    if (Math.abs(off) >= 1) deps.nudge(area, off);
    // The room past the rows' foot is all the list keeps below the fitted foot: the keyboard's
    // inset under the row and the list's own end padding, so a reader resting on the composer is
    // at the foot, and the keeper that takes a follower to the foot (`footTop`) stops where this
    // jump does instead of going on past it and being brought back (`#2277`). Measured here, once
    // the jump is made, so the foot's measures never pay for it per scroll.
    const below = (Number(area.scrollHeight) || 0) - (Number(area.scrollTop) || 0) - (Number(area.clientHeight) || 0);
    onScreenKeyboard.fittedRoom = Math.max(onScreenKeyboard.inset, Math.round(below));
    // For the readout: where the jump took the foot, and how far it has moved the list in all.
    const last = onScreenKeyboard.lastFit;
    const before = last && last.field === field.id && onScreenKeyboard.settling ? last.moved : 0;
    onScreenKeyboard.lastFit = { target: Math.round(target), moved: before + Math.round(off), field: field.id };
  }

  /** Redraw the readout every half second while it is drawn, rescheduling itself as the page's polls do. */
  function tickKeyboardDebug(): void {
    keyboardDebugTimer = 0;
    if (!keyboardDebug) return;
    renderKeyboardDebug();
    keyboardDebugTimer = Number(deps.clock.setTimeout(tickKeyboardDebug, 500));
  }

  function showKeyboardDebug(on: boolean): void {
    keyboardDebug = Boolean(on);
    (deps.el("keyboard-debug") as HTMLInputElement).checked = keyboardDebug;
    if (keyboardDebug && !keyboardDebugTimer) keyboardDebugTimer = Number(deps.clock.setTimeout(tickKeyboardDebug, 500));
    if (!keyboardDebug && keyboardDebugTimer) {
      deps.clock.clearTimeout(keyboardDebugTimer as number);
      keyboardDebugTimer = 0;
    }
    renderKeyboardDebug();
  }

  /**
   * Draw the readout, where it is on: one line each for the window, the visual viewport, the box and its
   * list, and the last jump, in CSS pixels as the page reads them. Moved down to the visual viewport's top
   * in the document (`pageTop`), since Safari's pan, and any scroll of the document it makes, leave the
   * layout viewport's top, where the readout is placed, above the screen.
   */
  function renderKeyboardDebug(): void {
    const readout = deps.lookup("keyboard-debug-readout");
    if (!readout) return;
    if (!keyboardDebug) {
      if (!readout.hidden) readout.hidden = true;
      return;
    }
    const n = (value: unknown): string => (typeof value === "number" && Number.isFinite(value) ? String(Math.round(value)) : "-");
    const viewport = deps.viewport();
    const field = deps.activeElement();
    const box = field && KEYBOARD_FIT_PARTS[field.id] ? field : deps.lookup("channel-compose-text");
    // The foot and the list as the jump measures them while the keyboard is up; the box and the list it is
    // in otherwise, so the numbers before the keyboard are there to compare.
    const where = composerFit(box);
    const area = where ? where.area : ["scroll-area", "side-scroll"].map((id) => deps.lookup(id)).find((list) => nodeWithin(list, box));
    const rect = (node: Element | null | undefined): DOMRect | null => (node && node.getClientRects().length ? node.getBoundingClientRect() : null);
    const boxRect = rect(box);
    const footRect = where ? rect(where.foot) : boxRect;
    const areaRect = rect(area);
    const top = viewport ? Number(viewport.offsetTop) || 0 : 0;
    const keysTop = viewport ? top + (Number(viewport.height) || 0) : deps.innerHeight();
    const last = onScreenKeyboard.lastFit;
    const root = deps.root();
    readout.textContent = [
      `keyboard ${root.getAttribute("data-keyboard") || "-"}  box ${box ? box.id : "-"}` +
        `${field === box ? " (focus)" : ""}  settling ${onScreenKeyboard.settling ? "yes" : "no"}` +
        `  reader moved ${onScreenKeyboard.readerMoved ? "yes" : "no"}`,
      `innerHeight ${n(deps.innerHeight())}  clientHeight ${n(root.clientHeight)}` +
        `  scrollY ${n(deps.scrollY())}`,
      viewport
        ? `visualViewport height ${n(viewport.height)}  offsetTop ${n(top)}  pageTop ${n(viewport.pageTop)}` +
          `  scale ${typeof viewport.scale === "number" ? viewport.scale.toFixed(2) : "-"}`
        : "visualViewport none",
      `keys top ${n(keysTop)}  box top ${n(boxRect && boxRect.top)} bottom ${n(boxRect && boxRect.bottom)}` +
        `  foot ${where && where.foot !== box ? where.foot.id : "box"} ${n(footRect && footRect.bottom)}` +
        `  foot - keys ${n(footRect ? footRect.bottom - keysTop : undefined)}`,
      `list ${area ? area.id : "-"} top ${n(areaRect && areaRect.top)} bottom ${n(areaRect && areaRect.bottom)}` +
        `  scrollTop ${n(area && area.scrollTop)} of ${n(area && area.scrollHeight - area.clientHeight)}` +
        `  room ${n(onScreenKeyboard.inset)}`,
      last ? `last jump ${last.field}: target ${last.target}, list moved ${last.moved} in all` : "last jump none",
    ].join("\n");
    // The document's own scroll too, where Safari scrolls it to show the field: `pageTop` is both.
    const pageTop = viewport && typeof viewport.pageTop === "number" ? viewport.pageTop : top + (deps.scrollY() || 0);
    readout.style.transform = pageTop ? `translateY(${Math.round(pageTop)}px)` : "";
    if (readout.hidden) readout.hidden = false;
  }

  return {
    noteKeyboard,
    noteKeyboardReader,
    settleOnScroll() {
      if (onScreenKeyboard.settling && onScreenKeyboard.fitted) settleKeyboardFit(onScreenKeyboard.fitted);
      // `#1680 inline-reply-keyboard-clutter`. The browser's own reveal scroll, which shows
      // the focused box as the keyboard comes up, moves the row's Send after the measure
      // that stood the corner Send: say again which Send stands, from where the row now is.
      if (onScreenKeyboard.cornerField) {
        renderCornerSend(onScreenKeyboard.shown, deps.activeElement(), deps.viewport());
      }
    },
    noteComposerInput(field) {
      if (onScreenKeyboard.fitted !== field || onScreenKeyboard.inputDue) return;
      onScreenKeyboard.inputDue = true;
      deps.clock.setTimeout(() => {
        onScreenKeyboard.inputDue = false;
        if (onScreenKeyboard.fitted === field) settleKeyboardFit(field);
      }, 0);
    },
    roomIn(area) {
      return onScreenKeyboard.fitted !== null && area === onScreenKeyboard.fittedArea
        ? onScreenKeyboard.fittedRoom : 0;
    },
    fittedFoot(composer) {
      const fitted = onScreenKeyboard.fitted;
      const parts = fitted ? KEYBOARD_FIT_PARTS[fitted.id] : undefined;
      if (!fitted || !parts || deps.lookup(parts[0]) !== composer) return null;
      return composerFoot(composer);
    },
    showKeyboardDebug,
    renderKeyboardDebug,
    pressCornerSend,
  };
}

// What the page and the tests use of it.
export { KEYBOARD_FIT_PARTS, KEYBOARD_MIN_PX, KEYBOARD_MIN_SHARE, KEYBOARD_SEND_PARTS, KEYBOARD_SETTLE_MS };

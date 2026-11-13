// Settings' Screen section: the text size and the reading width (`#55 voice-desktop-app`, `#230
// text-size-buttons`; `#624 voice-page-modularity`, moved from the legacy script; plan step 16,
// view/settings).
//
// On a wide screen the page holds its two lists to a column instead of letting them fill the window,
// and the reader chooses how wide that column is. Two controls set the one value — the handle on the
// edge of the column and the slider in Settings — because the handle needs a mouse and a keyboard needs
// something it can tab to.
//
// WHICH LAYOUT IS IN FORCE IS DECIDED ENTIRELY IN CSS, by `@media (min-width: 900px) and (pointer:
// fine)` at the foot of web/voice.css. Nothing here asks `matchMedia`, and nothing here looks at a
// user-agent string: this code only ever sets a number, and the stylesheet decides whether that number
// means anything. That is what keeps one regime rather than two that can disagree, and it is why the
// handle can be wired unconditionally. (`#1398 reading-width-handle-gate` adds one measured
// exception, below: whether the message column itself is wide enough to offer narrowing is a
// fact about that column's rendered box, which only a measurement of the column can say.)
//
// The unit is CHARACTERS, not pixels. What makes a line hard to read is how many characters are on it,
// so that is what the reader is choosing and what gets stored. The type size is a PERCENTAGE of the
// stylesheet's own, bounded (web/src/core/settings): under about eighty per cent the text stops being
// readable at arm's length on a phone, and over about a hundred and fifty the fold control and the row's
// own controls no longer line up with the words they belong to.

import { DEFAULT_READING_CH, MAX_MSG_SCALE, MAX_READING_CH, MIN_MSG_SCALE, clampMsgScale, clampReadingWidth } from "../../core/settings/index.ts";
import { gripKeysFor, msgScaleStepTarget, msgScaleWords, readingWidthWords } from "../../present/settings/index.ts";

/** What a setting's control asks of the page: the element by id, and the device's copy of the value. */
export interface ScreenSettingDependencies {
  /** The element with this id (`el`). */
  lookup(id: string): HTMLElement;
  /** Keep the value on the device; may throw where storage is refused. */
  keep(value: number): void;
  /** Whether the device holds exactly this value now: private browsing accepts a write and keeps nothing. */
  keptAs(value: number): boolean;
}

/** What the stored screen values ask of the page: the device's copies, already clamped. */
export interface StoredScreenDependencies {
  /** The stored type size. */
  msgScale(): number;
  /** The stored reading width. */
  readingWidth(): number;
}

/** The screen values as stored: what a restore puts on the page. */
export interface StoredScreen {
  /** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
  storedMsgScale(): number;
  /** What was stored, clamped. A missing or corrupt entry is the default, never an error. */
  storedReadingWidth(): number;
}

/** The stored copies, read where the page restores its settings. */
export function createStoredScreen(deps: StoredScreenDependencies): StoredScreen {
  return {
    storedMsgScale: () => deps.msgScale(),
    storedReadingWidth: () => deps.readingWidth(),
  };
}

/** The text size's named commands. */
export interface TextSize {
  /** Put a type size on the page, and return the one actually applied, which is the clamped one. */
  apply(value: unknown): number;
  /** Apply a new size and say, in Settings, whether it will survive a reload. */
  changed(value: unknown): void;
  /** One step of the slider, down (-1) or up (+1): what its − and + do. */
  step(direction: 1 | -1): void;
}

/** The text size, with nothing of its own to hold: the page and the device hold it. */
export function mountTextSize(deps: ScreenSettingDependencies): TextSize {
  /**
   * The − and + beside the slider are disabled at the end each moves towards, here, so that they are
   * right after a drag and at load as well as after a press of one of them.
   */
  function apply(value: unknown): number {
    const pct = clampMsgScale(value);
    document.documentElement.style.setProperty("--msg-scale", String(pct / 100));
    (deps.lookup("msg-scale") as HTMLInputElement).value = String(pct);
    (deps.lookup("msg-scale-smaller") as HTMLButtonElement).disabled = pct <= MIN_MSG_SCALE;
    (deps.lookup("msg-scale-larger") as HTMLButtonElement).disabled = pct >= MAX_MSG_SCALE;
    return pct;
  }

  /** Store it, and READ IT BACK: private browsing accepts `setItem` and keeps nothing. */
  function persist(pct: number): boolean {
    try {
      deps.keep(pct);
    } catch (_error) {
      return false;
    }
    return deps.keptAs(pct);
  }

  function changed(value: unknown): void {
    const pct = apply(value);
    deps.lookup("msg-scale-state").textContent = msgScaleWords(pct, persist(pct));
  }

  return {
    apply,
    changed,
    /**
     * `#230 text-size-buttons`. Through `changed`, the slider's own path, so a press is applied, kept
     * and said exactly as a drag to the same place is. A held button does not repeat: one press, one
     * step, and the slider is there for a big move.
     */
    step(direction) {
      const target = msgScaleStepTarget(clampMsgScale((deps.lookup("msg-scale") as HTMLInputElement).value), direction);
      const button = deps.lookup(direction > 0 ? "msg-scale-larger" : "msg-scale-smaller") as HTMLButtonElement;
      const held = document.activeElement === button;
      changed(target);
      // A button disabled under the focus drops it, and the keyboard is left on nothing. The slider
      // between the two takes it, which says the size it is at and moves on with the arrows.
      if (held && button.disabled) deps.lookup("msg-scale").focus();
    },
  };
}

/** The reading width's named commands, and the handle's. */
export interface ReadingWidth {
  /** Put a width on the page. Returns the width that was actually applied, which is the clamped one. */
  apply(value: unknown): number;
  /** Apply a new width and say, in the settings screen, whether it will survive a reload. */
  changed(value: unknown): void;
  /** The handle pressed: a drag begins. */
  gripDown(event: PointerEvent | undefined): void;
  /** The handle dragged: the width follows it, not yet kept. */
  gripMove(event: PointerEvent | undefined): void;
  /** The handle let go: the width it stopped at is kept. */
  gripUp(): void;
  /** A key on the handle. */
  gripKey(event: KeyboardEvent | undefined): void;
}

/**
 * `#1398 reading-width-handle-gate`. The column width past which the handle is offered at
 * all, in characters: the widest width the setting itself allows (`MAX_READING_CH`, 120ch).
 * A column no wider than that has nothing to gain from being artificially narrowed — it
 * reads best at full width — so below this width there is no handle and the remembered
 * narrower width is kept on the device but not applied. Only a column measured wider
 * than this (a wide monitor, or a wide column beside a second column or a docked panel)
 * earns the choice. The comparison is against the message column's own rendered width,
 * never the viewport's: docking a panel or splitting the screen squeezes the column
 * without moving the window.
 */
export const READING_GRIP_THRESHOLD_CH = MAX_READING_CH;

/**
 * Whether the handle shows for a column `columnWidthPx` wide at `pxPerCh` pixels per
 * character. An unmeasured column (nothing laid out yet, or a width that is not a
 * number) keeps the handle: hiding a control on a guess is worse than showing one
 * that a measurement a frame later will place.
 */
export function readingGripShown(columnWidthPx: number, pxPerCh: number): boolean {
  if (!Number.isFinite(columnWidthPx) || !Number.isFinite(pxPerCh) || columnWidthPx <= 0 || pxPerCh <= 0) {
    return true;
  }
  return columnWidthPx > READING_GRIP_THRESHOLD_CH * pxPerCh;
}

/** The width actually applied for a remembered one: full width while the handle is gated off. */
export function effectiveReadingWidth(rememberedCh: number, shown: boolean): number {
  return shown ? rememberedCh : MAX_READING_CH;
}

/**
 * Where the handle stands, in pixels from the main screen's left edge: on the message
 * column's own right edge (`column`), never at the screen's middle, so a docked panel
 * or a second column beside it is never stood over.
 */
export function readingGripLeft(column: { right: number }, screen: { left: number }): number {
  return column.right - screen.left;
}

/** The default width in force, and no drag. Made where the page kept the width. */
export function mountReadingWidth(deps: ScreenSettingDependencies): ReadingWidth {
  // The width currently in force, kept here rather than read back out of the DOM: reading a custom
  // property back gives you the string that was written, and a drag would then be parsing its own
  // output forty times a second.
  let readingWidth = DEFAULT_READING_CH;
  // The width actually on the page: the remembered one while the handle is shown, full
  // width (the widest allowed, which cannot cap a column under the threshold) while it
  // is not. The slider and the handle keep saying the remembered one throughout.
  let appliedWidth = DEFAULT_READING_CH;
  // Whether the measured column earns the handle. Unmeasured until the first layout
  // says otherwise, and shown until then, as `readingGripShown` keeps it.
  let gripShown = true;
  // Pixels per character, last measured off the column on screen (see `pixelsPerCh`).
  let perCh = 8;
  // A drag, in three events. Pointer events rather than mouse events so a trackpad, a pen and a
  // desktop touchscreen all work through one path; the regime that shows the handle at all is the
  // stylesheet's business, not this code's.
  let widthDrag: { centre: number; perCh: number } | null = null;
  /** The handle's keys, each the width it moves to from the one in force. */
  const gripKeys = gripKeysFor(() => readingWidth);

  /**
   * The custom property goes on `document.documentElement` because the two things it has to reach —
   * the panes inside #screen-main and the control pane inside the dock — have no common ancestor
   * below the root.
   */
  function apply(value: unknown): number {
    readingWidth = clampReadingWidth(value);
    appliedWidth = effectiveReadingWidth(readingWidth, gripShown);
    document.documentElement.style.setProperty("--reading-width", `${appliedWidth}ch`);
    (deps.lookup("reading-width") as HTMLInputElement).value = String(readingWidth);
    deps.lookup("width-grip").setAttribute("aria-valuenow", String(readingWidth));
    deps.lookup("width-grip").setAttribute("aria-valuetext", `${readingWidth} characters`);
    placeGrip();
    return readingWidth;
  }

  /**
   * Measure the message column and put the handle where the measurement says: on the
   * column's (`#pane-voice`, inside `#scroll-area`) own right edge, shown only while
   * that column is wider than the threshold. Called on every apply and again whenever
   * the column changes size — a resize, a dock or undock, a second column coming or
   * going all change the column's box without asking this code — so the decision is
   * always the current layout's. A column that cannot be measured yet leaves the
   * handle as it stands.
   */
  /**
   * How many pixels one character is, measured from the font itself: ten zeros in the
   * shown pane's own font, laid out hidden and taken away again. `1ch` is the advance
   * of a zero, so this is the conversion whatever the font and size — and, unlike the
   * pane's own width, it stays true while the pane spans its column uncapped (two
   * columns, `#373`) or stands at full width under the gate. 0 when it cannot be
   * measured here, and the caller keeps its last figure.
   */
  function measurePerCh(): number {
    try {
      if (typeof getComputedStyle !== "function") return 0;
      const paneEl = deps.lookup("pane-voice");
      const style = getComputedStyle(paneEl);
      const probe = document.createElement("span");
      probe.textContent = "0000000000";
      probe.style.position = "absolute";
      probe.style.visibility = "hidden";
      probe.style.whiteSpace = "pre";
      probe.style.padding = "0";
      probe.style.border = "0";
      probe.style.fontFamily = style.fontFamily;
      probe.style.fontSize = style.fontSize;
      probe.style.fontWeight = style.fontWeight;
      probe.style.letterSpacing = style.letterSpacing;
      document.body.appendChild(probe);
      const width = probe.getBoundingClientRect().width;
      probe.remove();
      return width > 0 ? width / 10 : 0;
    } catch (_error) {
      return 0;
    }
  }

  /** The pane on screen now: the transcript's or the channel's, whichever has a box. Both are
   * held to the reading width, and the handle belongs to the one being read. */
  function shownPane(): { rect: DOMRect } | null {
    for (const id of ["pane-voice", "pane-discord"]) {
      try {
        const rect = deps.lookup(id).getBoundingClientRect();
        if (rect.width > 0) return { rect };
      } catch (_error) { /* not on this page: the other pane may be */ }
    }
    return null;
  }

  function placeGrip(): void {
    let area: DOMRect | undefined;
    let screen: DOMRect | undefined;
    const paneBox = shownPane();
    try {
      area = deps.lookup("scroll-area").getBoundingClientRect();
      screen = deps.lookup("screen-main").getBoundingClientRect();
    } catch (_error) {
      return; // Nothing laid out to measure: the handle keeps its last place.
    }
    const pane = paneBox?.rect;
    if (!area || !pane || !screen || !(area.width > 0) || !(pane.width > 0)) return;
    const measuredCh = measurePerCh();
    if (measuredCh > 0) perCh = measuredCh;
    const screenEl = deps.lookup("screen-main");
    const maximized = screenEl.getAttribute("data-discuss") === "maximized";
    const shown = !maximized && readingGripShown(area.width, perCh);
    const grip = deps.lookup("width-grip") as HTMLElement;
    grip.style.left = `${readingGripLeft(pane, screen)}px`;
    // Said on the screen as well as on the handle: in two columns the stylesheet
    // re-caps the main pane only while this says "shown" (`#373` spans it otherwise).
    screenEl.setAttribute("data-reading-grip", shown ? "shown" : "hidden");
    if (shown === gripShown) {
      grip.hidden = !shown;
      return;
    }
    gripShown = shown;
    grip.hidden = !shown;
    // The gate flipped: re-apply so a squeezed column goes full width at once and a
    // widened one takes its remembered width back. The remembered value itself is
    // untouched either way, and nothing is stored for a change of room.
    appliedWidth = effectiveReadingWidth(readingWidth, gripShown);
    document.documentElement.style.setProperty("--reading-width", `${appliedWidth}ch`);
  }

  /**
   * Store it, and READ IT BACK rather than assuming — the same check the microphone settings make, for
   * the same reason: private browsing accepts setItem and stores nothing, so "saved" would be a lie
   * discovered only after a reload.
   */
  function persist(ch: number): boolean {
    try {
      deps.keep(ch);
    } catch (_error) {
      return false;
    }
    return deps.keptAs(ch);
  }

  function changed(value: unknown): void {
    const ch = apply(value);
    deps.lookup("reading-width-state").textContent = readingWidthWords(ch, persist(ch));
  }

  /** The conversion a drag moves by: the font's own measure, kept current. */
  function pixelsPerCh(): number {
    const measured = measurePerCh();
    if (measured > 0) perCh = measured;
    return perCh; // nothing laid out yet; the last measure (or a plausible 8) beats dividing by zero.
  }

  // Re-place the handle whenever the column's box changes or the layout around it is
  // re-decided: a window resize, Discuss docking, undocking or maximizing, and a
  // second column coming or going all reach the column through one of these two.
  // Neither observer exists in every test double, and neither is needed for the
  // width itself — the stylesheet and `apply` carry that — so both are best-effort.
  try {
    if (typeof ResizeObserver === "function") {
      const watcher = new ResizeObserver(() => placeGrip());
      watcher.observe(deps.lookup("scroll-area"));
      watcher.observe(deps.lookup("pane-voice"));
      watcher.observe(deps.lookup("pane-discord"));
    }
  } catch (_error) { /* no observer: applies still place the handle */ }
  try {
    if (typeof MutationObserver === "function") {
      const layoutWatcher = new MutationObserver(() => placeGrip());
      layoutWatcher.observe(deps.lookup("screen-main"), { attributes: true, attributeFilter: ["data-discuss", "data-columns"] });
    }
  } catch (_error) { /* no observer: applies still place the handle */ }

  return {
    apply,
    changed,
    gripDown(event) {
      // The column, not the screen: its box is the drag's frame, so a docked panel or a
      // second column beside it cannot pull the width being set. The column is centred
      // in its area, so the handle's distance from the area's middle is HALF the width.
      // Captured once at the start of the drag: re-measuring mid-drag would feed the
      // drag its own output.
      let box: DOMRect | undefined;
      try {
        box = deps.lookup("scroll-area").getBoundingClientRect();
        if (!(box.width > 0)) box = deps.lookup("screen-main").getBoundingClientRect();
      } catch (_error) {
        box = deps.lookup("screen-main").getBoundingClientRect();
      }
      widthDrag = { centre: box.left + box.width / 2, perCh: pixelsPerCh() };
      const grip = deps.lookup("width-grip");
      if (event && event.pointerId !== undefined && grip.setPointerCapture) {
        // So the drag survives the pointer leaving the nine-pixel handle, which it does immediately.
        grip.setPointerCapture(event.pointerId);
      }
    },
    gripMove(event) {
      if (!widthDrag || !event) {
        return;
      }
      apply(((event.clientX - widthDrag.centre) * 2) / widthDrag.perCh);
    },
    gripUp() {
      if (!widthDrag) {
        return;
      }
      widthDrag = null;
      // Stored at the END of the drag, not on every frame: forty writes a second to localStorage is a
      // synchronous disk hit per frame, and only the width the reader stopped at is a decision.
      changed(readingWidth);
    },
    gripKey(event) {
      const next = event && gripKeys[event.key];
      if (!next) {
        return;
      }
      if (event.preventDefault) {
        event.preventDefault(); // PageUp and the arrows would otherwise scroll the list behind it.
      }
      changed(next());
    },
  };
}

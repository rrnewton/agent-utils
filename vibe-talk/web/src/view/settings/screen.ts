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
// handle can be wired unconditionally.
//
// The unit is CHARACTERS, not pixels. What makes a line hard to read is how many characters are on it,
// so that is what the reader is choosing and what gets stored. The type size is a PERCENTAGE of the
// stylesheet's own, bounded (web/src/core/settings): under about eighty per cent the text stops being
// readable at arm's length on a phone, and over about a hundred and fifty the fold control and the row's
// own controls no longer line up with the words they belong to.

import { DEFAULT_READING_CH, MAX_MSG_SCALE, MIN_MSG_SCALE, clampMsgScale, clampReadingWidth } from "../../core/settings/index.ts";
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

/** The default width in force, and no drag. Made where the page kept the width. */
export function mountReadingWidth(deps: ScreenSettingDependencies): ReadingWidth {
  // The width currently in force, kept here rather than read back out of the DOM: reading a custom
  // property back gives you the string that was written, and a drag would then be parsing its own
  // output forty times a second.
  let readingWidth = DEFAULT_READING_CH;
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
    document.documentElement.style.setProperty("--reading-width", `${readingWidth}ch`);
    (deps.lookup("reading-width") as HTMLInputElement).value = String(readingWidth);
    deps.lookup("width-grip").setAttribute("aria-valuenow", String(readingWidth));
    deps.lookup("width-grip").setAttribute("aria-valuetext", `${readingWidth} characters`);
    return readingWidth;
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

  /**
   * How many pixels one character is, measured from the column that is actually on screen rather
   * than assumed. `1ch` is the width of a zero in the current font, which is neither a constant nor
   * knowable from here — but the pane IS `max-width: var(--reading-width)`, so its rendered width
   * divided by the width in force is the conversion factor, whatever the font turns out to be.
   */
  function pixelsPerCh(): number {
    const measured = deps.lookup("pane-voice").getBoundingClientRect().width;
    if (measured > 0 && readingWidth > 0) {
      return measured / readingWidth;
    }
    return 8; // nothing laid out yet; a plausible figure beats dividing by zero.
  }

  return {
    apply,
    changed,
    gripDown(event) {
      const box = deps.lookup("screen-main").getBoundingClientRect();
      // The column is centred, so the handle's distance from the middle is HALF the width. Captured
      // once at the start of the drag: re-measuring mid-drag would feed the drag its own output.
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

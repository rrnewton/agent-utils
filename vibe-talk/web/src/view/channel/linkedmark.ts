// The linked message's mark (`#890 voice-page-linked-highlight`; `#462 webapp-deep-links`): a page opened on a link
// to a message, or a link followed in it, keeps that message marked (an accent edge, a tinted surface, and a small
// "Linked message" note with a × on it) until the reader puts the mark out: Esc where nothing else takes it, the ×,
// or the next link, which moves it. Neither the selection nor the mouse nor a scroll nor a list drawn again touches
// it: it is a state of its own, kept here by message and channel and drawn again on every row drawn holding it.

/** The linked message: its channel and id. */
export interface LinkedMessage {
  readonly channel: string;
  readonly id: string;
}

export interface LinkedMarkDependencies {
  /** Every row drawn now, in every list, cards included. */
  rowsDrawn(): readonly HTMLElement[];
  /** The ids of the messages `row` holds: for a card, its root's. */
  idsOf(row: HTMLElement): readonly string[];
  /** The channel the lists show. */
  channel(): string;
  /** Whether `row` is on screen, in a list that is. */
  rowShown(row: HTMLElement): boolean;
  /** The reader put out the mark on `linked` (Esc or the ×): not another link moving it, nor signing out. */
  dismissed(linked: LinkedMessage): void;
}

/** The mark's commands and questions. */
export interface LinkedMark {
  /** The linked message, or null. */
  linked(): LinkedMessage | null;
  /** Mark `id` in `channel`: the rows holding it now, and every one drawn later. The old mark goes. */
  set(channel: string, id: string): void;
  /** Put the mark out. Returns whether there was one. */
  clear(): boolean;
  /** Mark `row` if it holds the linked message, or unmark it if it no longer does: for every row as it is drawn. */
  drawOn(row: HTMLElement, ids?: readonly string[]): void;
  /** Esc: put out a mark drawn on screen. Returns whether there was one; a mark out of sight lets Esc go on. */
  clearOnEscape(): boolean;
}

/** The note's words. */
export const LINKED_MARK_WORDS = {
  note: "Linked message",
  clear: "Stop highlighting the linked message",
} as const;

const MARKER_CLASS = "linked-marker";
/** What leads a row ahead of its note: a reply's arrow and a thread's strip. */
const LEADING = ["reply-jump", "thread-strip"];

export function createLinkedMark(deps: LinkedMarkDependencies): LinkedMark {
  let linked: LinkedMessage | null = null;
  /** The rows drawn with the mark, to put it out on. */
  const marked = new Set<HTMLElement>();

  function markerOf(row: HTMLElement): HTMLElement | null {
    return (Array.from(row.children) as HTMLElement[]).find((child) => child.className === MARKER_CLASS) || null;
  }

  function unmark(row: HTMLElement): void {
    row.removeAttribute("data-linked");
    const marker = markerOf(row);
    if (marker) row.removeChild(marker);
    marked.delete(row);
  }

  function marker(): HTMLElement {
    const note = document.createElement("span");
    note.className = MARKER_CLASS;
    const words = document.createElement("span");
    words.className = "linked-marker-words";
    words.textContent = LINKED_MARK_WORDS.note;
    const clear = document.createElement("button");
    clear.className = "linked-marker-clear";
    clear.setAttribute("type", "button");
    clear.setAttribute("aria-label", LINKED_MARK_WORDS.clear);
    clear.setAttribute("title", LINKED_MARK_WORDS.clear);
    clear.textContent = "×";
    clear.addEventListener("click", () => {
      dismiss();
    });
    note.append(words, clear);
    return note;
  }

  /** Put the mark out for the reader, and say so. */
  function dismiss(): void {
    const was = linked;
    if (api.clear() && was) deps.dismissed(was);
  }

  /** Where the note goes: after a reply's arrow and a thread's strip, which lead the row (`threadStrip`). */
  function noteAt(row: HTMLElement): Element | null {
    const kids = Array.from(row.children);
    return kids.find((kid) => !String(kid.className).split(/\s+/).some((name) => LEADING.includes(name))) || null;
  }

  function holds(ids: readonly string[]): boolean {
    return linked !== null && deps.channel() === linked.channel && ids.includes(linked.id);
  }

  const api: LinkedMark = {
    linked: () => linked,
    set(channel, id) {
      api.clear();
      linked = { channel: String(channel), id: String(id) };
      for (const row of deps.rowsDrawn()) api.drawOn(row);
    },
    clear() {
      const had = linked !== null;
      linked = null;
      for (const row of Array.from(marked)) unmark(row);
      // And any row in a list still drawn with it: one let go of above, before it was put in its list.
      for (const row of deps.rowsDrawn()) {
        if (row.hasAttribute("data-linked")) unmark(row);
      }
      return had;
    },
    drawOn(row, ids = deps.idsOf(row)) {
      if (!holds(ids)) {
        if (row.hasAttribute("data-linked")) unmark(row);
        return;
      }
      row.setAttribute("data-linked", "true");
      if (!markerOf(row)) row.insertBefore(marker(), noteAt(row));
      // Rows drawn before this one and taken out of their list since are let go.
      for (const one of Array.from(marked)) {
        if (one !== row && !one.parentNode) marked.delete(one);
      }
      marked.add(row);
    },
    clearOnEscape() {
      if (!Array.from(marked).some((row) => deps.rowShown(row))) return false;
      dismiss();
      return true;
    },
  };
  return api;
}

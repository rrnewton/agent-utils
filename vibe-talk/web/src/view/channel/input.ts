// What the reader used last, keys, mouse or a finger, and where the mouse last moved (`#239
// single-selection`; `#624 voice-page-modularity`, moved from the legacy script into view/channel): the one
// owner of that, saying it on the root element as `data-input` for web/voice.css, and telling a mouse really
// moving from a list scrolled under a pointer standing still. What a selection does with it is the page's.

/** What the reader used last: "keys", "mouse", "touch" for a finger or a pen, or "" before any of them. */
export type InputKind = "" | "keys" | "mouse" | "touch";

/** What the owner asks of the page. */
export interface InputKindsDependencies {
  /** The root element, which says the keys or a finger (`document.documentElement`). */
  root(): HTMLElement;
}

/** The owner's commands and questions. */
export interface InputKinds {
  /**
   * What the reader used last. A selection made by a click, a jump or the focus arriving on a row is
   * drawn after the keys or the mouse, and silent after a touch or anything unknown (`drawnBy`).
   */
  kind(): InputKind;
  /** Whether a selection made now is drawn, with a ring and the focus: after the keys or the mouse. */
  drawnBy(): boolean;
  /**
   * The keys are in use: noted, and said on the root element as `data-input="keys"` until the mouse
   * really moves. web/voice.css holds back what a pointer resting over a list would light while it is
   * there, so a key that scrolls the list under a still pointer lights nothing it did not select.
   */
  keys(): void;
  /** The mouse is in use: noted, and the keys' `data-input` taken off the root element. */
  mouse(): void;
  /**
   * A finger or a pen is in use: noted, and said on the root as `data-input="touch"` until the mouse
   * moves or a key is pressed. A touch laptop has a mouse too, and answers the pointer media queries as
   * a desk does, so its finger would lose the focus tint a touch screen keeps; web/voice.css gives the
   * tint to the attribute as well.
   */
  touch(): void;
  /** Where the mouse was at its last move, in the viewport, or null before the first. */
  mouseAt(): { readonly x: number; readonly y: number } | null;
  /**
   * Whether a `pointermove` is a mouse really moving: a mouse's, somewhere other than the last one
   * was. A list scrolled under a pointer standing still (by a key, or by a message arriving) makes some
   * browsers report a move to put `:hover` right, and it reports the same place; a finger or a pen is
   * not a mouse. With no move before it to compare, the first counts, unless the keys are in use and it
   * says it did not move: that is the one a key's scroll can make before the mouse has moved at all.
   */
  mouseMoved(event: { readonly pointerType?: string; readonly clientX?: unknown; readonly clientY?: unknown;
    readonly movementX?: unknown; readonly movementY?: unknown } | null | undefined): boolean;
}

/** Nothing used yet. */
export function createInputKinds(deps: InputKindsDependencies): InputKinds {
  let inputKind: InputKind = "";
  let mouseAt: { x: number; y: number } | null = null;
  return {
    kind: () => inputKind,
    drawnBy: () => inputKind === "keys" || inputKind === "mouse",
    keys() {
      inputKind = "keys";
      const root = deps.root();
      if (root.getAttribute("data-input") !== "keys") root.setAttribute("data-input", "keys");
    },
    mouse() {
      inputKind = "mouse";
      const root = deps.root();
      if (root.hasAttribute("data-input")) root.removeAttribute("data-input");
    },
    touch() {
      inputKind = "touch";
      const root = deps.root();
      if (root.getAttribute("data-input") !== "touch") root.setAttribute("data-input", "touch");
    },
    mouseAt: () => mouseAt,
    mouseMoved(event) {
      if (!event || event.pointerType !== "mouse") return false;
      const x = Number(event.clientX);
      const y = Number(event.clientY);
      const was = mouseAt;
      mouseAt = { x, y };
      if (was === null) return inputKind !== "keys" || Boolean(event.movementX || event.movementY);
      return was.x !== x || was.y !== y;
    },
  };
}

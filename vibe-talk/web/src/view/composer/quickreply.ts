// The quoted words' and the quick-reply box's listeners (`#267 quote-reply`, `#726 vt-selection-chip`, `#727
// vt-selection-quick-reply`, `#819 voice-page-quoted-reply-and-draft-chip`, `#821 voice-page-read-aloud-react`, `#829
// voice-page-dictation-polish`, `#846 draft-chip-honest`; `#624 voice-page-modularity`, moved from the legacy script's
// wiring into view/composer for the plan's step 17): words selected in a message, the chips that quote them, the
// floating box's keys, Send and Cancel and its question, its microphone, the Draft chip and React, a tap outside
// that puts the box down, and the lists and the window that move it. What each does is the page's; mounted once,
// in the page's order, and taken down again.

import { createListening, nodeWithin } from "../dom/index.ts";

export interface QuickReplyListenersDependencies {
  /** An element of the page by id, and one of the left column's (the chips and the paused read's strip). */
  byId(id: string): HTMLElement;
  leftById(id: string): HTMLElement;
  /** Words selected in one message make its Reply a Quote reply. */
  noteQuoteSelection(): unknown;
  /** The selection chip, or its twin in the paused read's strip, pressed. */
  pressSelectionChip(): unknown;
  /** The box's Send, Cancel, and its question answered. */
  sendQuickReply(): unknown;
  cancelQuickReply(): unknown;
  answerQuickReplyDiscard(discard: boolean): unknown;
  /** Close the box, its words kept. */
  closeQuickReply(): unknown;
  /** `#1315 floating-reply-dock`. The dock icon: the draft goes into the reply in the stream, focused. */
  dockQuickReply(): unknown;
  /** Any other key in the box: the message boxes' own keys. */
  onBoxKey(event: KeyboardEvent): unknown;
  /** Watch what is pasted or dropped into the box. */
  watchBoxInsertions(box: HTMLElement): unknown;
  /** What is written in the box, kept as it is written. */
  noteQuickReplyInput(): unknown;
  /** While dictation listens: the keyboard asked back, and listening stopped. */
  showKeyboard(): unknown;
  stopDictation(): unknown;
  /** The Draft chip, and React in the paused read's strip, pressed. */
  pressDraftChip(): unknown;
  reactToHeldSentence(): unknown;
  /** Whether the box is open; putting it down puts what was written back under its message. */
  quickReplyOpen(): boolean;
  putDownQuickReply(): boolean;
  /** Take the click a put-down tap ends in, so it presses nothing else (`rowGestures.claimClick`). */
  claimClick(): unknown;
  /** A list scrolled, and the box placed again as the window's visible part changes. */
  quickReplyListMoved(target: EventTarget | null): unknown;
  visualViewport(): EventTarget | null;
  placeQuickReply(): unknown;
  /** Whether words are held, and the chip placed over them again. */
  selectionHeld(): boolean;
  placeSelectionChip(): unknown;
}

/** The listeners, mounted. */
export interface QuickReplyListeners {
  /** Take every listener down again. */
  unmount(): void;
}

/** A press that takes neither the selection nor the focus from the words it quotes. */
function keepSelection(event: Event): void {
  event.preventDefault();
}

/** Mount the quoted words' and the quick-reply box's listeners. */
export function mountQuickReplyListeners(deps: QuickReplyListenersDependencies): QuickReplyListeners {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  const left = (id: string): HTMLElement => deps.leftById(id);
  listen<Event>(document, "selectionchange", () => deps.noteQuoteSelection());
  // `#726 vt-selection-chip`, and its twin in the paused read's strip (`#819`).
  for (const id of ["selection-chip", "read-steer-quote"]) {
    listen<Event>(left(id), "mousedown", keepSelection);
    listen<Event>(left(id), "click", () => deps.pressSelectionChip());
  }
  listen<Event>(el("quick-reply-dock"), "click", () => deps.dockQuickReply());
  listen<Event>(el("quick-reply-send"), "click", () => deps.sendQuickReply());
  listen<Event>(el("quick-reply-cancel"), "click", () => deps.cancelQuickReply());
  listen<Event>(el("quick-reply-discard-yes"), "click", () => deps.answerQuickReplyDiscard(true));
  // `#846 draft-chip-honest`. Esc on the question is Esc on the box: it closes, its words kept, and goes no further.
  for (const id of ["quick-reply-discard-yes", "quick-reply-discard-no"]) {
    listen<KeyboardEvent>(el(id), "keydown", (event) => {
      if (!event || event.key !== "Escape" || event.isComposing) return;
      if (typeof event.preventDefault === "function") event.preventDefault();
      if (typeof event.stopPropagation === "function") event.stopPropagation();
      deps.closeQuickReply();
    });
  }
  listen<Event>(el("quick-reply-discard-no"), "click", () => deps.answerQuickReplyDiscard(false));
  const box = el("quick-reply-text");
  listen<KeyboardEvent>(box, "keydown", (event) => {
    if (event && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      deps.closeQuickReply();
      return undefined;
    }
    return deps.onBoxKey(event);
  });
  deps.watchBoxInsertions(box);
  // `#819`. Kept on the device as it is written, dictation included.
  listen<Event>(box, "input", () => deps.noteQuickReplyInput());
  // `#829 voice-page-dictation-polish`. A tap in the box while it listens asks for the keyboard back; a tap on the
  // microphone badge stops listening, the focus staying in the box.
  listen<Event>(box, "pointerdown", () => deps.showKeyboard());
  listen<Event>(el("quick-reply-mic"), "mousedown", keepSelection);
  listen<Event>(el("quick-reply-mic"), "click", () => deps.stopDictation());
  listen<Event>(left("draft-chip"), "click", () => deps.pressDraftChip());
  // `#821 voice-page-read-aloud-react`. React, in the paused read's strip.
  listen<Event>(el("read-react"), "click", () => deps.reactToHeldSentence());
  // A tap outside the box and the chips that open it puts the box down, heard before anything it lands on.
  // `#1402 docked-cancel-putdown`. A tap that put a reply down claims the click behind it, as a swipe
  // or a hold does (`rowGestures.claimClick`): the press is the put-down's, and its click does not also
  // unfold the tall folded row it landed on, which would carry the box just opened below the list's foot.
  let putDownPress = false;
  listen<PointerEvent>(document, "pointerdown", (event) => {
    if (!deps.quickReplyOpen()) return;
    const target = (event && event.target) as Node | null;
    if (target && (nodeWithin(el("quick-reply"), target) || nodeWithin(left("selection-chip"), target) ||
      nodeWithin(left("read-steer-quote"), target) || nodeWithin(left("draft-chip"), target) ||
      nodeWithin(left("read-react"), target))) return;
    putDownPress = deps.putDownQuickReply();
  }, { capture: true });
  listen<PointerEvent>(document, "pointerup", () => {
    if (!putDownPress) return;
    putDownPress = false;
    deps.claimClick();
  }, { capture: true });
  listen<PointerEvent>(document, "pointercancel", () => {
    putDownPress = false;
  }, { capture: true });
  listen<Event>(document, "scroll", (event) => deps.quickReplyListMoved(event && event.target), { capture: true, passive: true });
  const viewport = deps.visualViewport();
  if (viewport && typeof viewport.addEventListener === "function") {
    listen<Event>(viewport, "resize", () => deps.placeQuickReply());
  }
  // A list scrolled under the selection carries the words, and the chip goes with them.
  listen<Event>(document, "scroll", () => { if (deps.selectionHeld()) deps.placeSelectionChip(); }, { capture: true, passive: true });
  return { unmount };
}

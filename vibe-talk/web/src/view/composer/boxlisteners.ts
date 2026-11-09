// A message box's listeners (`#225 enter-to-send`, `#241 voice-composer`, `paste-never-sends`; `#624
// voice-page-modularity`, moved from the legacy script's wiring into view/composer for the plan's step 17): what is
// written kept as it is written, Send pressed, the box's keys, and what is pasted or dropped into it. The call's box,
// the channel's (a thread's is the same box posting somewhere else), and its twin in the right column all mount the
// same way; what each send does is the page's. Mounted once, in the page's order, and taken down again.

import { createListening } from "../dom/index.ts";

export interface MessageBoxDependencies {
  /** The box. */
  box: HTMLElement;
  /** Its Send button and what it does, and what is written kept as it is written, where the box has them. */
  sendButton?: HTMLElement;
  send?(): unknown;
  remember?(): unknown;
  /**
   * Text went into the box (`input`), where the box has it (`#1144 scrollback-typing-jump`):
   * at the foot, the keyboard's jump is made whole after the browser reveals the caret.
   */
  typed?(): unknown;
  /** Its Cancel and the Yes and No of the question Cancel asks, where the box has them (`#1135 raw-message-page`). */
  cancelButton?: HTMLElement;
  discardYes?: HTMLElement;
  discardNo?: HTMLElement;
  cancel?(): unknown;
  answerDiscard?(discard: boolean): unknown;
  /** A key in the box: one rule for every message box (`onComposerKey`), with this box's send. */
  onKey(event: KeyboardEvent): unknown;
  /** `paste-never-sends`. A paste only ever puts text in. */
  watchBoxInsertions(box: HTMLElement): unknown;
}

/** The listeners, mounted. */
export interface MessageBox {
  /** Take every listener down again. */
  unmount(): void;
}

/** Mount a message box's listeners: what is written, Send, the keys, then what is pasted, in the page's order. */
export function mountMessageBox(deps: MessageBoxDependencies): MessageBox {
  const { listen, unmount } = createListening();
  const remember = deps.remember;
  if (remember) listen<Event>(deps.box, "input", () => remember());
  const typed = deps.typed;
  if (typed) listen<Event>(deps.box, "input", () => typed());
  const send = deps.send;
  if (deps.sendButton && send) listen<Event>(deps.sendButton, "click", () => send());
  const cancel = deps.cancel;
  if (deps.cancelButton && cancel) listen<Event>(deps.cancelButton, "click", () => cancel());
  const answerDiscard = deps.answerDiscard;
  if (deps.discardYes && answerDiscard) listen<Event>(deps.discardYes, "click", () => answerDiscard(true));
  if (deps.discardNo && answerDiscard) listen<Event>(deps.discardNo, "click", () => answerDiscard(false));
  listen<KeyboardEvent>(deps.box, "keydown", (event) => deps.onKey(event));
  deps.watchBoxInsertions(deps.box);
  return { unmount };
}

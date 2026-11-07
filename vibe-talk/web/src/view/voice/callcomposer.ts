// The call's message box and its canned prompts (`#241 voice-composer`, `#60 canned-prompt-buttons`, `#193
// compose-draft-loss`; `#624 voice-page-modularity`, moved from the legacy script's wiring into view/voice for the
// plan's step 17): Send, the box's draft restored and kept as it is written, a tap that takes the box into view, and
// each canned prompt's field restored and kept and its button. What each does is the call's; mounted once, in the
// page's order, and taken down again. The box's keys and pastes are `mountMessageBox`'s.

import { createListening } from "../dom/index.ts";

/** A canned prompt as its listeners read it: its field, its button, and the key its text is kept under. */
export interface CannedPromptEntry {
  readonly field: string;
  readonly button: string;
  readonly key: string;
}

export interface CallComposerDependencies<Entry extends CannedPromptEntry> {
  /** An element of the page by id. */
  lookup(id: string): HTMLElement;
  /** Send what is written (`sendTyped`). */
  sendTyped(): unknown;
  /** The box's draft as kept, a tap in the box, and what is written kept as it is written. */
  storedComposeDraft(): string;
  composerFocused(): unknown;
  composeInput(): unknown;
  /** The canned prompts, their texts as kept, a field changed, and a button pressed. */
  cannedPrompts: readonly Entry[];
  storedCannedPrompts(): Record<string, string>;
  cannedPromptsChanged(): unknown;
  pressCannedPrompt(entry: Entry): unknown;
}

/** The listeners, mounted. */
export interface CallComposer {
  /** Take every listener down again. */
  unmount(): void;
}

/** Restore the call's box and the canned prompts' fields, and mount their listeners, in the page's order. */
export function mountCallComposer<Entry extends CannedPromptEntry>(deps: CallComposerDependencies<Entry>): CallComposer {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.lookup(id);
  listen<Event>(el("send-text"), "click", () => deps.sendTyped());
  (el("compose-text") as HTMLTextAreaElement).value = deps.storedComposeDraft();
  // `#241 voice-composer`. A tap on the part of the box that shows takes the whole of it into view.
  listen<Event>(el("compose-text"), "focus", () => deps.composerFocused());
  listen<Event>(el("compose-text"), "input", () => deps.composeInput());
  // `#60 canned-prompt-buttons`. ONE loop over the list: the field is restored, the field is saved when it changes,
  // and the button sends whatever the field says. A third canned prompt is one more entry in `CANNED_PROMPTS` and one
  // more pair of elements in web/voice.html — there is deliberately no third place to remember.
  //
  // The defaults are written into `.value` HERE rather than typed into web/voice.html, exactly as `#api-token` is:
  // text inside a <textarea> in the markup is its child text, and `.value` and the markup would then be two different
  // answers to "what does this button send".
  for (const entry of deps.cannedPrompts) {
    (el(entry.field) as HTMLTextAreaElement).value = deps.storedCannedPrompts()[entry.key] as string;
    // `input` as well as `change`: `change` waits for the field to lose focus, and a phone that kills the app while
    // the field still has it never gets one. `#193 compose-draft-loss`.
    listen<Event>(el(entry.field), "input", () => deps.cannedPromptsChanged());
    listen<Event>(el(entry.field), "change", () => deps.cannedPromptsChanged());
    listen<Event>(el(entry.button), "click", () => deps.pressCannedPrompt(entry));
  }
  return { unmount };
}

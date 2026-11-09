// The message boxes: growth, and in time focus, draft presentation and send controls. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS, createComposerGrow } from "./grow.ts";
export type { ComposerGrow, ComposerGrowDependencies } from "./grow.ts";
export { createComposerBoxes } from "./boxes.ts";
export type { ComposerBox, ComposerBoxes, ComposerBoxesDependencies } from "./boxes.ts";
export { mountComposerGrowListeners } from "./growlisteners.ts";
export type { ComposerGrowListeners, ComposerGrowListenersDependencies } from "./growlisteners.ts";
export { mountMessageBox } from "./boxlisteners.ts";
export type { MessageBox, MessageBoxDependencies } from "./boxlisteners.ts";
export {
  COMPOSER_FIELDS,
  NEWLINE_KEYS,
  PASTE_ENTER_MS,
  createComposerKeys,
  focusAtEnd,
  insertNewline,
  isComposer,
  watchBoxInsertions,
  wireNewlineKey,
} from "./keys.ts";
export type { ComposerKeyEvent, ComposerKeys, ComposerKeysDependencies } from "./keys.ts";
export { KEYBOARD_FIT_PARTS, KEYBOARD_MIN_PX, KEYBOARD_MIN_SHARE, KEYBOARD_SETTLE_MS, createOnScreenKeyboard } from "./keyboard.ts";
export type { OnScreenKeyboard, OnScreenKeyboardDependencies } from "./keyboard.ts";
export { DICTATION_LISTENING, DICTATION_UNAVAILABLE, createDictation } from "./dictation.ts";
export type { Dictation, DictationDependencies, Recognition, RecognitionResultEvent } from "./dictation.ts";
export { mountKeyboardListeners } from "./keyboardlisteners.ts";
export type { KeyboardListeners, KeyboardListenersDependencies } from "./keyboardlisteners.ts";
export { mountQuickReplyListeners } from "./quickreply.ts";
export type { QuickReplyListeners, QuickReplyListenersDependencies } from "./quickreply.ts";
export { createChannelSend } from "./send.ts";
export type { ChannelSend, ChannelSendDependencies } from "./send.ts";

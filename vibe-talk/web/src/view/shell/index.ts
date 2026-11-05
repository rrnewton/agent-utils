// view/shell (`#624 voice-page-modularity`): what stands over every screen, and in time the top-level screen
// mounting, keyboard dispatch and boot order (the plan's step 17). Its public interface.

export { BANNER_DISMISS_MS, STATUS_DISMISS_MS, createShellNotices } from "./notices.ts";
export type { ShellNotices, ShellNoticesDependencies } from "./notices.ts";
export { CHIPS_PILL_REM, createChipsOverComposer } from "./chips.ts";
export type { ChipsOverComposer, ChipsOverComposerDependencies } from "./chips.ts";
export { createUndoOffer, createUndoStamps } from "./undo.ts";
export type { UndoOffer, UndoStamps } from "./undo.ts";
export { SCREENS, createScreens } from "./screens.ts";
export type { Screens } from "./screens.ts";
export { ACTION_MENU_ITEMS, createActionMenu } from "./actionmenu.ts";
export { createRowShortcuts } from "./rowshortcuts.ts";
export type { RowShortcuts, ShortcutButton } from "./rowshortcuts.ts";
export { createChannelPick } from "./channelpick.ts";
export type { ChannelPick } from "./channelpick.ts";
export type { ActionMenu, ActionMenuDependencies } from "./actionmenu.ts";
export { MAC_FIELD_EDIT_LETTERS, appleKeyboard, chordMatches, isLetterKey, modHeld, swallow, typedKey, typesCharacter } from "./chords.ts";
export type { KeyChord, KeyEvent } from "./chords.ts";
export { createReplySight } from "./draftsight.ts";
export type { ReplySight } from "./draftsight.ts";
export { createPromptsTray } from "./promptstray.ts";
export type { PromptsTray, PromptsTrayDependencies } from "./promptstray.ts";
export { createKeyDispatch } from "./keydispatch.ts";
export type { DispatchedKey, KeyBinding, KeyDispatch, KeyDispatchDependencies } from "./keydispatch.ts";
export { KEY_WORDS, chordShortcuts, chordWords, createKeyHints, viewKeysNote } from "./keyhints.ts";
export type { KeyHints, KeyHintsDependencies, SaidBinding } from "./keyhints.ts";
export { COPIED_MESSAGE, COPIED_SELECTION, NOTHING_TO_COPY, createHighlightCopy, htmlText, plainParagraphsHtml } from "./highlightcopy.ts";
export type { CopiedPane, CopyEvent, HighlightCopy, HighlightCopyDependencies, MessageCopy } from "./highlightcopy.ts";
export { SCROLL_KEYS, createScrollKeys, pageStep } from "./scrollkeys.ts";
export type { ScrollKeyEvent, ScrollKeys, ScrollKeysDependencies, ScrolledPane } from "./scrollkeys.ts";
export { createScreenKeys } from "./screenkeys.ts";
export type { KeyPlace, ScreenKeys, ScreenKeysDependencies } from "./screenkeys.ts";
export { createComposeKeys } from "./composekeys.ts";
export type { ComposeKeys, ComposeKeysDependencies } from "./composekeys.ts";

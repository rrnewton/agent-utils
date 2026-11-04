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

// The message boxes: growth, and in time focus, draft presentation and send controls. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS, createComposerGrow } from "./grow.ts";
export type { ComposerGrow, ComposerGrowDependencies } from "./grow.ts";
export { COMPOSER_FIELDS, NEWLINE_KEYS, PASTE_ENTER_MS, createComposerKeys, insertNewline, watchBoxInsertions, wireNewlineKey } from "./keys.ts";
export type { ComposerKeyEvent, ComposerKeys, ComposerKeysDependencies } from "./keys.ts";

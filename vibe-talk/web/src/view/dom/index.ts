// The page's DOM primitives: small walks and, in time, the shell's typed element lookup. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { childByClass, nodeWithin, shownNode } from "./tree.ts";
export { elementIn, focusLost, focusMinimally, setFocusReveal } from "./shell.ts";
export type { FocusReveal } from "./shell.ts";
export type { ElementById } from "./ids.ts";
export { classWords, setClassWord } from "./classes.ts";
export { sayingNodes, setAttributeIfChanged, setClassNameIfChanged, setHiddenIfChanged, setTextIfChanged, tagNameOf } from "./changes.ts";
export { UNTYPED_INPUTS, answersEnter, isLink, isTextField, keepsOwnKeys, pressedBySpace, scrollsItself } from "./targets.ts";
export { copyText, copyThroughTextarea, createPendingCopy } from "./clipboard.ts";
export { createOncePerFrame } from "./frames.ts";
export type { FrameScheduler, OncePerFrame } from "./frames.ts";
export type { PendingCopy, TextClipboard } from "./clipboard.ts";
export { createListening } from "./listening.ts";
export { insertSanitizedHtml } from "./html.ts";
export type { Listening, SizeWatcher, SizeWatching } from "./listening.ts";
export { foldSectionKeys, foldSectionsByHeading, openSectionKeys, setAllSections } from "./collapse.ts";

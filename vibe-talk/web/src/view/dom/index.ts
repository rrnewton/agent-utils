// The page's DOM primitives: small walks and, in time, the shell's typed element lookup. Its public
// interface. `#624 voice-page-modularity`, plan step 16.

export { childByClass, nodeWithin, shownNode } from "./tree.ts";
export { elementIn, focusMinimally } from "./shell.ts";
export type { ElementById } from "./ids.ts";
export { classWords, setClassWord } from "./classes.ts";
export { sayingNodes, setAttributeIfChanged, setClassNameIfChanged, setHiddenIfChanged, setTextIfChanged, tagNameOf } from "./changes.ts";
export { UNTYPED_INPUTS, answersEnter, isLink, isTextField, keepsOwnKeys, pressedBySpace, scrollsItself } from "./targets.ts";

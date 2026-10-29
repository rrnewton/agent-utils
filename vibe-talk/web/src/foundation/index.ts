// Stateless helpers every part of the page can use: text, identifiers and time, with no state and
// no browser. Its public interface. `#624 voice-page-modularity`.

export { MONO_EM, SPACE_EM, WIDE_SCRIPT, glyphEm, monoEm } from "./glyphs.ts";
export { linkKind } from "./links.ts";
export type { LinkKind } from "./links.ts";
export { sentenceStart } from "./text.ts";
export { DEEP_LINK_PART_MAX, deepLinkRefFrom, deepLinkRefText, messageLinkRef } from "./refs.ts";
export type { DeepLinkRef, MessageLinkRef } from "./refs.ts";
export { createSerialQueue } from "./queue.ts";
export type { SerialQueue } from "./queue.ts";

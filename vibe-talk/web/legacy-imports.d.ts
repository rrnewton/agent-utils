// The names the page build's prelude declares for web/voice.js: exactly what
// web/src/legacy/imports.ts exports, with the types the modules give them. `#624
// voice-page-modularity`. A test in tests/js/page_build.test.mjs holds this list to that file's.

type LegacyImports = typeof import("./src/legacy/imports.ts");

declare const searchPositiveTerms: LegacyImports["searchPositiveTerms"];
declare const searchQueryMatches: LegacyImports["searchQueryMatches"];
declare const searchQueryParse: LegacyImports["searchQueryParse"];
declare const searchQueryTree: LegacyImports["searchQueryTree"];
declare const searchSubject: LegacyImports["searchSubject"];
declare const readPieces: LegacyImports["readPieces"];
declare const sentenceEnds: LegacyImports["sentenceEnds"];
declare const speechSegments: LegacyImports["speechSegments"];
declare const createApi: LegacyImports["createApi"];
declare const createWebPorts: LegacyImports["createWebPorts"];
declare const SPACE_EM: LegacyImports["SPACE_EM"];
declare const WIDE_SCRIPT: LegacyImports["WIDE_SCRIPT"];
declare const glyphEm: LegacyImports["glyphEm"];
declare const linkKind: LegacyImports["linkKind"];
declare const monoEm: LegacyImports["monoEm"];
declare const DEEP_LINK_PART_MAX: LegacyImports["DEEP_LINK_PART_MAX"];
declare const deepLinkRefFrom: LegacyImports["deepLinkRefFrom"];
declare const deepLinkRefText: LegacyImports["deepLinkRefText"];
declare const messageLinkRef: LegacyImports["messageLinkRef"];

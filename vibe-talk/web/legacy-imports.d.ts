// The names the page build's prelude declares for web/voice.js: exactly what
// web/src/legacy/imports.js exports, with the types the modules give them. `#624
// voice-page-modularity`. A test in tests/js/page_build.test.mjs holds this list to that file's.

type LegacyImports = typeof import("./src/legacy/imports.js");

declare const searchPositiveTerms: LegacyImports["searchPositiveTerms"];
declare const searchQueryMatches: LegacyImports["searchQueryMatches"];
declare const searchQueryParse: LegacyImports["searchQueryParse"];
declare const searchQueryTree: LegacyImports["searchQueryTree"];
declare const searchSubject: LegacyImports["searchSubject"];

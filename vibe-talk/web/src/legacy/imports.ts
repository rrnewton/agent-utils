// What the legacy page script (web/voice.js) takes from the modules, and nothing else.
//
// `#624 voice-page-modularity`. The page's code moves out of web/voice.js into these modules a
// piece at a time; what is left there is still a classic script, and still calls what moved. The
// page build (web/build.mjs) bundles this file into a strict prelude that runs just before
// web/voice.js and declares exactly these names for it, so the legacy script reads them as it read
// its own functions before. Name only what voice.js really uses: a name the legacy script no longer
// needs comes off this list, and the list is empty when the legacy script is gone.
// web/legacy-imports.d.ts declares the same names for `tsc --checkJs` over voice.js; a test holds
// the two lists equal.

export {
  searchPositiveTerms,
  searchQueryMatches,
  searchQueryParse,
  searchQueryTree,
  searchSubject,
} from "../core/search/index.ts";
export { readPieces, sentenceEnds, speechSegments } from "../core/speech/index.ts";

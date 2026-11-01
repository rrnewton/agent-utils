// present/call (`#624 voice-page-modularity`): what a voice call shows and says — a turn's words as the
// transcript draws them, the sentence its agent is told about a draft's outcome, and the line for a call
// that ended. Presentation decisions with no DOM: the voice view draws what these say.

export { closeReason, outcomeNotice } from "./notices.ts";
export { CUT_OFF_MARK, containsWords, mergeHypothesis, revisionDistance, shownSpoken, spokenText, spokenWords, tidySpeech } from "./transcript.ts";
export type { SpokenEntry } from "./transcript.ts";

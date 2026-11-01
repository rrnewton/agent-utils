// present/call (`#624 voice-page-modularity`): what a voice call shows and says — a turn's words as the
// transcript draws them, the sentence its agent is told about a draft's outcome, and the line for a call
// that ended. Presentation decisions with no DOM: the voice view draws what these say.

export { closeReason, outcomeNotice } from "./notices.ts";
export { CUT_OFF_MARK, containsWords, mergeHypothesis, revisionDistance, shownSpoken, spokenText, spokenWords, tidySpeech } from "./transcript.ts";
export type { SpokenEntry } from "./transcript.ts";
export {
  READ_BACK_LEAD_INS,
  SENDING_QUESTION_WORDS,
  SENDING_WORDS,
  SEND_DESTINATION_WORDS,
  SHORT_ANSWER_WORDS,
  SPOKEN_NO_STARTS,
  SPOKEN_YES_ACKNOWLEDGEMENT,
  SPOKEN_YES_FILLER,
  SPOKEN_YES_HINTS,
  SPOKEN_YES_PHRASES,
  agentDraft,
  answerWords,
  askedAboutSending,
  classifySpokenAnswer,
  couldBeSpokenYes,
  isExactReadBack,
  isSpokenYes,
  isSpokenYesAcknowledgement,
  mayBeYes,
  namesWhereToSend,
  readBackRefusal,
  turnAsksAboutSending,
  unsayableDraft,
  yesPrefix,
} from "./answers.ts";

export { CLOUD_CAUSES, CLOUD_NO_AUDIO, CLOUD_SILENT, READ_NO_SPEECH_MS, cloudFailureText } from "./read-notices.ts";

export { postKey, sentWhere, sentNotes, proposalFate, restoredSpeaker, oldestFirst } from "./records.ts";

export { relayTask, relayTurn, relayCount } from "./relay.ts";

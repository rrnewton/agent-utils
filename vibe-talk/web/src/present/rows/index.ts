// present/rows (`#624 voice-page-modularity`): how the page words and arranges its rows of messages —
// what a fold hides, the lines between rows, a reaction's face. Presentation decisions with no DOM:
// view code draws what these decide.

export {
  ELIDED_MIN_CHARS,
  ELIDED_MIN_WORDS,
  SPACE_POOR_CHARS,
  UNDO_NAME_CHARS,
  cardGapWords,
  cardMaxLinesWords,
  cardRunText,
  collapsedLinesWords,
  elisionLabel,
  gapWords,
  lineLabel,
  lineWords,
  reactionKey,
  reactionText,
  readRunText,
  undoLabel,
  undoName,
} from "./labels.ts";
export type { ReactionFace } from "./labels.ts";
export { ELISION_COUNTED_TEXTS, FOLD_WIDTHS_EM, NO_ELISION_LABELS, elisionLabels, foldPieces, foldShows, forgetElisionCounts } from "./fold.ts";
export type { FoldLaid, FoldLine, FoldPiece, FoldRun } from "./fold.ts";
export { GLOM_WINDOW_MS, isNoise, joinsGroup } from "./grouping.ts";
export { READ_RUN_MIN_ROWS, createReadRuns } from "./runs.ts";
export type { ReadRuns, RunPlace } from "./runs.ts";
export {
  CARD_INDENTED_CHARS,
  CARD_LINE_CHARS,
  cardProxyLines,
  firstLineOf,
  foldedCardText,
  proxyLinesOf,
  threadChoice,
  threadFacts,
  wrapCardLine,
} from "./cards.ts";
export type { ThreadChoice } from "./cards.ts";
export { THREAD_HUE_APART, THREAD_HUE_BESIDE, hueApart, newestThreadHues, spreadThreadHue } from "./hues.ts";
export { BLANK_TEXT, messageLook, rawTextOf, renderedHtmlOf } from "./message.ts";
export { GLOM_SEPARATOR, combinedContent, editedDrawn, reactionsDrawn, rowEdited, rowReactions } from "./rowdata.ts";
export type { ReactionTally } from "./rowdata.ts";
export { QUOTE_MAX_CHARS, quoteBlock, quoteMarksRead, quoteShare, quotedHead } from "./quote.ts";
export { countOf, replyMarkerName } from "./replies.ts";
export { mainRunsAmong } from "./mainruns.ts";
export { seamPlan } from "./seams.ts";
export type { Seam, SeamPlan, SeamRow } from "./seams.ts";
export {
  CARD_STATES,
  THREADS_UNPACKED_KEY,
  THREAD_CARD_STATES_KEPT,
  THREAD_CARD_STATES_KEY,
  UNPACK_MODES,
  UNPACK_MODE_NAMES,
  UNPACK_MODE_SAID,
  createCardPacking,
} from "./packing.ts";
export type { CardPacking, CardPackingDependencies } from "./packing.ts";
export { bookmarkSpot } from "./bookmark.ts";
export type { BookmarkAnchorFacts, BookmarkRow } from "./bookmark.ts";
export {
  READ_MODES,
  READ_MODE_KEY,
  READ_MODE_LABELS,
  READ_MODE_SAID,
  READ_MODE_TITLES,
  READ_MODE_VERBS,
  createReadMode,
  nextReadMode,
  savedReadMode,
} from "./readmode.ts";
export type { ReadMode, ReadModeDependencies } from "./readmode.ts";
export {
  SUMMARY_FAILED_MARK,
  SUMMARY_MARK,
  SUMMARY_MODE_OFF,
  SUMMARY_MODE_ON,
  SUMMARY_WAITING,
  summaryFailureWords,
  summaryNoteWords,
  typicalSummaryMs,
} from "./summaries.ts";
export {
  AUTHOR_FILTERS,
  AUTHOR_FILTER_CHIP_LABELS,
  AUTHOR_FILTER_LABELS,
  AUTHOR_FILTER_TITLES,
  authorEmptyWords,
  nextAuthorFilter,
} from "./authorfilter.ts";
export { LINK_KINDS, LINK_KINDS_KEY, LINK_KIND_COUNT_MAX, createLinkKinds, hiddenKindWords, linkKindCountText } from "./linkkinds.ts";
export type { LinkKind, LinkKinds, LinkKindsDependencies } from "./linkkinds.ts";

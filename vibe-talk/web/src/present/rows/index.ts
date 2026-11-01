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

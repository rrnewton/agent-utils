// view/channel (`#624 voice-page-modularity`): the channel's rows, cards, replies and columns as the DOM
// draws them. It calls present/* for what to say and how to arrange it, and core/* for what is held.
// Moving in from the legacy script a piece at a time; see the plan's step 16.

export { FOLD_FLOW_BLOCKS, FOLD_INLINE, RESTATED_CLASS, elisionLine, foldRuns, foldedTextLine, textNodesOf } from "./fold.ts";
export { DAY_TITLED, EDITED_MARK, freshenDayTitle, rowTime, rowTimeTabStop, timeNode } from "./time.ts";
export { TEXT_BLOCKS, anchorsAmong, findableParts, hitRanges, textHits, textRunsOf } from "./hits.ts";
export {
  anchorOfRow,
  authorLabel,
  doneButtonOf,
  idsOf,
  isMessageRow,
  messageTop,
  notNoiseButtonOf,
  rowIn,
  rowMessages,
  rowNewestMs,
  rowSentMs,
  rowShown,
  shownRowFor,
  standingRow,
} from "./rows.ts";
export type { RowElement } from "./rows.ts";
export { renderedText } from "./text.ts";
export {
  CARD_HEADING_TAGS,
  CARD_LIST_TAGS,
  CARD_TEXT_TAGS,
  CARD_WHOLE_TAGS,
  cardMarkTarget,
  cardStubText,
  cardUnits,
  keepCardUnits,
  nodeWords,
  tableShape,
} from "./cardtext.ts";
export type { CardUnit } from "./cardtext.ts";
export { QUOTE_EDGE_NODES, drawnWordsBetween, quotableRow, replyRowOf, rowQuotedBy } from "./quotes.ts";
export { LINK_ROW_INSET_PX, drawLinkPath, laneOrder, linkAt, linkedRootThread, pointerOn, rowLinkY } from "./links.ts";
export type { DrawnLink } from "./links.ts";
export { fromGatheredThread, gutterPiece, headerLayout, quoteLabelFits, setArrowReach, setMarkerLift, stackUnder } from "./replies.ts";
export { cardGapGroups, cardGapTally, cardRootOf, cardRunIds, fillCardText, markCardLayout, rowSeen, threadCardId } from "./cards.ts";
export type { CardGapGroup, CardRow } from "./cards.ts";

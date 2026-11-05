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
export { createSeamLines } from "./seams.ts";
export type { GapLine, PlacedSeam, SeamLines, SeamLinesDependencies, SeamedRow } from "./seams.ts";
export { HOLD_MS, SWIPE_COMMIT_PX, SWIPE_START_PX, createRowGestures } from "./gestures.ts";
export type { GestureRow, PlayingReading, RowGestures, RowGesturesDependencies, Tapped, TappedBlock } from "./gestures.ts";
export { createBookmarkLines } from "./bookmark.ts";
export type { BookmarkLines, BookmarkLinesDependencies, BookmarkPlace, BookmarkedRow } from "./bookmark.ts";
export {
  QUICK_REPLY_GAP_PX,
  QUICK_REPLY_MAX_WIDTH_PX,
  QUOTE_HOLD_MS,
  QUOTE_REPLY_TITLE,
  SELECTION_BAR_ROOM_PX,
  SELECTION_CHIP_EDGE_PX,
  SELECTION_CHIP_GAP_PX,
  SELECTION_CHIP_HANDLES_PX,
  createQuoting,
} from "./quoting.ts";
export type { HeldQuote, QuickReplyDraftStore, QuickReplySend, QuoteRoute, QuotedRow, Quoting, QuotingDependencies } from "./quoting.ts";
export { PULL_ARM_PX, PULL_EDGE_SLACK_PX, PULL_LABELS, PULL_RESULT_MS, PULL_RISE_PX, PULL_WAIT_MS, createPullToRefresh } from "./pull.ts";
export type { PullFreshness, PullToRefresh, PullToRefreshDependencies, PullTouch } from "./pull.ts";
export {
  COLUMN_GLIDE_MS,
  GLIDE_REST_PX,
  LINK_DOT_PX,
  LINK_GLIDE_MARGIN_PX,
  LINK_HEAD_PX,
  LINK_LANE_PX,
  createColumnLinks,
  planColumnLinks,
} from "./columnlinks.ts";
export type { ColumnLinks, ColumnLinksDependencies, LinkPair, LinkPane, MeasuredLinks, PlannedLink } from "./columnlinks.ts";
export { AUTHOR_CHIP_MS, authorPasses, createAuthorFilter, rowIsMine } from "./authorfilter.ts";
export type { AuthorFilter, AuthorFilterDependencies } from "./authorfilter.ts";
export { createCardPlaces, neighbourStayed, rowNeighbours, sameCardPlace } from "./cardplace.ts";
export type { CardPlace, CardPlaces, CardPlacesDependencies, Neighbours } from "./cardplace.ts";
export { createInputKinds } from "./input.ts";
export type { InputKind, InputKinds, InputKindsDependencies } from "./input.ts";
export { THREAD_HUES_KEY, THREAD_HUES_PER_CHANNEL, THREAD_HUES_SEEDED_KEY, THREAD_HUES_SEEN_REFRESH_MS, THREAD_HUES_TOTAL, THREAD_HUES_VERSION, THREAD_HUE_NEIGHBOURS, THREAD_HUE_RECENT, createThreadHues } from "./hues.ts";
export type { ThreadHues, ThreadHuesDependencies } from "./hues.ts";
export { createPinSync } from "./pins.ts";
export type { PinSync, PinSyncDependencies } from "./pins.ts";
export { OVERSCROLL_IDLE_MS, OVERSCROLL_REST_MS, createFootRooms } from "./footroom.ts";
export type { FootRooms, FootRoomsDependencies } from "./footroom.ts";
export { createPackSlides } from "./packslide.ts";
export type { PackSlides, PackSlidesDependencies } from "./packslide.ts";
export { createScrollKeeping } from "./scrollkeep.ts";
export type { ScrollKeeping, ScrollKeepingDependencies, ScrollMark } from "./scrollkeep.ts";
export { createCardFlight } from "./cardflight.ts";
export type { CardFlight, CardFlightDependencies } from "./cardflight.ts";
export { createArming } from "./arming.ts";
export type { Arming, ArmingDependencies } from "./arming.ts";
export { createColumnWork } from "./columnwork.ts";
export type { ColumnWork, WorkedColumn } from "./columnwork.ts";
export { createLayoutShown } from "./layoutshown.ts";
export type { Layout, LayoutShown } from "./layoutshown.ts";
export { createCardsDrawn } from "./cardsdrawn.ts";
export type { CardsDrawn, KeptCards } from "./cardsdrawn.ts";
export { UNPACK_NEAR_PX, createUnpackWatch } from "./unpackwatch.ts";
export type { UnpackWatch, UnpackWatchDependencies } from "./unpackwatch.ts";
export { createUnderLists } from "./underlists.ts";
export type { MainUnderListNote, UnderListNote, UnderLists } from "./underlists.ts";
export { createRowDrawing } from "./rowdrawing.ts";
export type { RowDrawing } from "./rowdrawing.ts";
export { createFilterChanges } from "./filterchanges.ts";
export type { FilterChanges, FilterWalk } from "./filterchanges.ts";
export { createRowMenus } from "./rowmenu.ts";
export type { RowMenu, RowMenus } from "./rowmenu.ts";
export { createReplyState } from "./replystate.ts";
export { createSummaryWatch } from "./summarywatch.ts";
export type { SummaryWatch, SummaryWatchDependencies } from "./summarywatch.ts";
export { createLastPane } from "./lastpane.ts";
export type { LastPane } from "./lastpane.ts";
export type { ReplyRouting, ReplyState } from "./replystate.ts";
export { mountFilterChips } from "./filterchips.ts";
export type { FilterChips, FilterChipsDependencies, SearchKey } from "./filterchips.ts";
export { mountListWatch } from "./listwatch.ts";
export type { ListWatch, ListWatchDependencies } from "./listwatch.ts";
export { mountListKeepers } from "./listkeepers.ts";
export type { ListKeepers, ListKeepersDependencies } from "./listkeepers.ts";
export { mountListGestures } from "./listgestures.ts";
export type { ListGestures, ListGesturesDependencies, PulledTouch } from "./listgestures.ts";
export { mountListControls } from "./listcontrols.ts";
export type { ListControls, ListControlsDependencies } from "./listcontrols.ts";

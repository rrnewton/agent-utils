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
  LINK_DOT_PX,
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
export { createScrollKeeping, rowsBelowReadingPosition } from "./scrollkeep.ts";
export type { ScrollKeeping, ScrollKeepingDependencies, ScrollMark } from "./scrollkeep.ts";
export { createCardFlight } from "./cardflight.ts";
export type { CardFlight, CardFlightDependencies } from "./cardflight.ts";
export { createArming } from "./arming.ts";
export type { Arming, ArmingDependencies } from "./arming.ts";
export { COLUMN_IDS, createColumnWork, createColumns } from "./columnwork.ts";
export type { ColumnFreshMakers, ColumnWork, ColumnWorkPort, ColumnWorkReader, ColumnSearchCount, PageColumn, SidePageColumn, WorkedColumn } from "./columnwork.ts";
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
export { FOLLOW_REST_PX, mountListKeepers } from "./listkeepers.ts";
export type { ListKeepers, ListKeepersDependencies } from "./listkeepers.ts";
export { mountListGestures } from "./listgestures.ts";
export type { ListGestures, ListGesturesDependencies, PulledTouch } from "./listgestures.ts";
export { mountListControls } from "./listcontrols.ts";
export type { ListControls, ListControlsDependencies } from "./listcontrols.ts";
export { mountChannelViewControls } from "./viewcontrols.ts";
export type { ChannelViewControls, ChannelViewControlsDependencies } from "./viewcontrols.ts";
export { mountSideListControls } from "./sidelistcontrols.ts";
export type { SideListControls, SideListControlsDependencies } from "./sidelistcontrols.ts";
export { mountReadingJumps } from "./readingjumps.ts";
export type { ReadingJumps, ReadingJumpsDependencies } from "./readingjumps.ts";
export { createReplySession } from "./replysession.ts";
export type { ReplyColumnLike, ReplyDraftStoreLike, ReplyMessageLike, ReplySession, ReplySessionDependencies } from "./replysession.ts";
export { createReplyArrows, freshReplyArrows, REPLY_LANDING_GAP_PX } from "./replyarrows.ts";
export { createReplyGathering, freshGatherState, freshListState } from "./gathering.ts";
export { createReadingColumn } from "./readingcolumn.ts";
export { createSentRead } from "./sentread.ts";
export { createThreadEntry, freshEntryState } from "./threadentry.ts";
export { createTwoColumns } from "./twocolumns.ts";
export { createColumnSplitter, mountColumnSplitterControls } from "./splitter.ts";
export type { ColumnSplitter, ColumnSplitterControls, ColumnSplitterControlsDependencies, ColumnSplitterDependencies, SplitterPlace } from "./splitter.ts";
export { createSideCards } from "./sidecards.ts";
export { createFilterPlace, freshFilterState } from "./filterplace.ts";
export { createLiveState } from "./livestate.ts";
export { createDoneAct, freshDoneState } from "./doneact.ts";
export { createChannelViews } from "./channelviews.ts";
export { createReplyCounts } from "./replycounts.ts";
export { createPlacing } from "./placing.ts";
export { createOlderPages, freshOlderPages } from "./olderpages.ts";
export { createOpeningView } from "./openingview.ts";
export type { OpeningRecord, OpeningView, OpeningViewDependencies } from "./openingview.ts";
export type { OlderPages, OlderPagesDependencies, OlderPagesRecord } from "./olderpages.ts";
export type { Placing, PlacingDependencies } from "./placing.ts";
export type { ReplyCounts, ReplyCountsDependencies } from "./replycounts.ts";
export type { DismissalOffer, DoneAct, DoneActDependencies, DoneLedger, DoneState } from "./doneact.ts";
export type { LiveFollowSwitch, LiveState, LiveStateDependencies, LiveStorage } from "./livestate.ts";
export type { ChannelViews, ChannelViewsDependencies, HeldViewContext, ReaderPlaceResult, ThreadRecordLike, ViewSelection } from "./channelviews.ts";
export type { FilterAnchor, FilterHold, FilterMark, FilterPane, FilterPlace, FilterPlaceDependencies, FilterState } from "./filterplace.ts";
export type { CardParts, SideCard, SideCards, SideCardsDependencies, SideChoice, SideTally } from "./sidecards.ts";
export type { LayoutChoiceLike, LayoutColumn, LayoutName, LayoutShownLike, LeftColumnPlace, ListBefore, OneColumnView, ReaderPlace, TwoColumns, TwoColumnsDependencies } from "./twocolumns.ts";
export type { EntryLandingMark, EntryState, ThreadEntry, ThreadEntryDependencies } from "./threadentry.ts";
export type { SentRead, SentReadDependencies, SentReadOutboxEntry, ServiceMark } from "./sentread.ts";
export type { ReadingColumn, ReadingColumnDependencies } from "./readingcolumn.ts";
export type { Gathering, GatheringDependencies, GatherMessage, GatherState, GatherThreadChoice, ListState, ReplyLinks } from "./gathering.ts";
export type { ReplyArrows, ReplyArrowsDependencies, ReplyArrowsState, ReplyArrowRow, ReplyJumpTicket } from "./replyarrows.ts";
export { createColumnReplies } from "./columnreplies.ts";
export type { ColumnReplies, ColumnRepliesDependencies, ColumnReplyEntry } from "./columnreplies.ts";
export { mountReplyBox } from "./replybox.ts";
export type { ReplyBox, ReplyBoxDependencies } from "./replybox.ts";
export { mountKeptSends } from "./keptsends.ts";
export type { KeptSends, KeptSendsDependencies } from "./keptsends.ts";
export { mountSideControls } from "./sidecontrols.ts";
export type { GestureStart, SideControls, SideControlsDependencies } from "./sidecontrols.ts";
export { LINKED_MARK_WORDS, createLinkedMark } from "./linkedmark.ts";
export type { LinkedMark, LinkedMarkDependencies, LinkedMessage } from "./linkedmark.ts";
export { createTimelineView } from "./readcore.ts";
export type { TimelineAbandonedRead, TimelineColumn, TimelineColumnSteps, TimelineEntryState, TimelineFoldRead, TimelineLoadState, TimelineOlderPages, TimelineReadOptions, TimelineReaderLike, TimelineRows, TimelineSelection, TimelineView, TimelineViewDependencies } from "./readcore.ts";
export { COLLAPSE_OVER_CHARS, FOLD_LESS, FOLD_LISTS, FOLD_MORE, SCROLLBACK_PX, SUMMARY_LOOKAHEAD_PX, createFolding } from "./folding.ts";
export type { FoldEntry, FoldPane, Folding, FoldingDependencies, ScrollPlace, ScrollToolsPass } from "./folding.ts";
export { DISMISS_BATCH, createChannelRows } from "./channelrows.ts";
export type { ChannelRows, ChannelRowsDependencies } from "./channelrows.ts";
export { SEARCH_OLDER_HITS, SEARCH_OLDER_MS, SEARCH_OLDER_PAGES, SEARCH_OLDER_TAP_MS, mountOlderWalks } from "./olderwalks.ts";
export type { OlderWalkColumn, OlderWalks, OlderWalksDependencies, SearchWalk } from "./olderwalks.ts";
export { SEARCH_LISTS, createSearchRows } from "./searchrows.ts";
export type { SearchCount, SearchFieldLike, SearchRows, SearchRowsDependencies } from "./searchrows.ts";
export { createCardGaps } from "./cardgaps.ts";
export type { CardGaps, CardGapsDependencies } from "./cardgaps.ts";
export { createUnpackBlock } from "./unpackblock.ts";
export type { UnpackBlock, UnpackBlockDependencies } from "./unpackblock.ts";
export { createPackControls } from "./packcontrols.ts";
export type { CardChoice, PackControls, PackControlsDependencies } from "./packcontrols.ts";
export { createChannelRefresh } from "./refresh.ts";
export type { ChannelRefresh, ChannelRefreshDependencies, RefreshPoll, RefreshReads } from "./refresh.ts";
export { createLiveStream } from "./livestream.ts";
export type { LiveFrame, LiveStream, LiveStreamDependencies } from "./livestream.ts";
export { createRowLinks } from "./rowlinks.ts";
export type { LinkKindsLike, RowLinks, RowLinksDependencies } from "./rowlinks.ts";
export { createMessageBodies } from "./body.ts";
export type { MessageBodies, MessageBodyDependencies } from "./body.ts";
export { createRowDress } from "./rowdress.ts";
export type { DressPass, RowDress, RowDressDependencies } from "./rowdress.ts";
export { createCardLines } from "./cardlines.ts";
export type { CardCut, CardLines, CardLinesDependencies, CardMaxLinesLike, QuoteReadPercentLike } from "./cardlines.ts";
export { createSpeakers } from "./speakers.ts";
export type { IdentitiesLike, Speakers, SpeakersDependencies } from "./speakers.ts";
export { createReadingBookmark } from "./bookmark.ts";
export type { BookmarkAnchorLike, ReadingBookmark, ReadingBookmarkDependencies } from "./bookmark.ts";
export { createServerSearchCopy } from "./serversearch.ts";
export type { ServerSearch, ServerSearchDependencies } from "./serversearch.ts";
export { createPinnedMessages } from "./pins.ts";
export type { PinnedMessages, PinnedMessagesDependencies, PinsView } from "./pins.ts";
export { createOutboxRows } from "./outboxrows.ts";
export type { OutboxRows, OutboxRowsDependencies, OutgoingRowEntry } from "./outboxrows.ts";
export { createSendTakeBack } from "./takeback.ts";
export type { DismissalHold, DismissedHeld, SendTakeBack, SendTakeBackDependencies, TakeBackEntry } from "./takeback.ts";
export { THREAD_DIRECTORY_REFRESH_MS, THREAD_SELECT_LIMIT, THREAD_TITLE_CHARS, createThreadList } from "./threadlist.ts";
export type { ThreadList, ThreadListDependencies } from "./threadlist.ts";
export { createOfflineCache } from "./offlinecache.ts";
export type { OfflineCacheDependencies } from "./offlinecache.ts";
export { createReadRunLines } from "./readrunlines.ts";
export type { ReadRunLines, ReadRunLinesDependencies } from "./readrunlines.ts";
export { createThreadStrip } from "./threadstrip.ts";
export type { ThreadStrip, ThreadStripDependencies } from "./threadstrip.ts";
export { SCROLL_TRACE_MAX_ENTRIES, SCROLL_TRACE_WINDOW_MS, createScrollTrace } from "./scrolltrace.ts";
export type { ScrollTrace, ScrollTraceAnchor, ScrollTraceDependencies } from "./scrolltrace.ts";
export { createCardRefresh } from "./cardrefresh.ts";
export type { CardRefresh, CardRefreshDependencies } from "./cardrefresh.ts";
export { createChannelScope } from "./channelscope.ts";
export type { ChannelScope, ChannelScopeDependencies, MutableFailure } from "./channelscope.ts";
export { createFreshRead } from "./freshread.ts";
export type { FreshRead, FreshReadDependencies, FreshDecisionLike, ReadPayload } from "./freshread.ts";
export { createReadState } from "./readstate.ts";
export type { ReadState, ReadStateDependencies, ReadStatePage } from "./readstate.ts";
export { createPendingSends } from "./pendingsends.ts";
export type { PendingSends, PendingSendsDependencies, PendingThreadsLike } from "./pendingsends.ts";
export { LOADING_LINE_DELAY_MS, createChannelLoading } from "./channelloading.ts";
export type { ChannelLoading, ChannelLoadingDependencies } from "./channelloading.ts";
export { createHeldDone } from "./helddone.ts";
export type { HeldDone, HeldDoneDependencies } from "./helddone.ts";

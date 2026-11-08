// The keyboard composition (`#624 voice-page-modularity`, plan step 17, move 2): every list the
// keys move through and every owner the keys dispatch through, built in the page's order from
// the page's acts. The panes, the selection owners and the key owners are view/shell's,
// view/channel's and core/settings'; what each key does on a row, a channel or the screen is
// the page's, handed in as narrow acts. No act is called while this composition is built
// except where the owners built here already called one (the keymap's wording asks, and
// nothing else): every act that reads a later owner is a thunk, exactly as the legacy
// script's wiring was, so building early and calling late is unchanged.

import {
  KEYS_MESSAGE,
  KEYS_MOVING,
  KEYS_PAGE,
  KEYS_VIEW,
  KEYS_WRITING,
  createChannelKeys,
  createDoneKeys,
  createFoldKeys,
  createHighlightCopy,
  createKeyDispatch,
  createKeyHints,
  createKeySelection,
  createKeyTiming,
  createKeymap,
  createPointerSelection,
  createReadCollapse,
  createRowKeys,
  createRowShortcuts,
  createScreenKeys,
  createScrollKeys,
  createSelectionKeys,
  createViewKeys,
  createWriteKeys,
  keyTimingLine,
} from "../view/shell/index.ts";
import type {
  ChannelKeys,
  ChannelPicker,
  DoneKeys,
  FoldKeys,
  HighlightCopy,
  KeyDispatch,
  KeyHints,
  KeyPane,
  KeySelection,
  KeySelectionModel,
  KeyTiming,
  KeymapBinding,
  PointerSelection,
  ReadCollapse,
  RowKeys,
  RowShortcuts,
  ScreenKeys,
  Screens,
  ScrollKeys,
  SelectionKeys,
  SteppedChannel,
  SteppedMenu,
  UndoOfferStamp,
  ViewKeys,
  WriteKeys,
} from "../view/shell/index.ts";
import { createInputKinds, createLastPane } from "../view/channel/index.ts";
import type { InputKinds, LastPane } from "../view/channel/index.ts";
import { createSingleKeys } from "../core/settings/index.ts";
import type { DeviceSettings, SingleKeys } from "../core/settings/index.ts";

/** A column of the page, as the keyboard composition reads it. */
export interface KeyboardColumn {
  readonly name: string;
}

/** The right column, with the mode the side pane and the row keys ask about. */
export interface KeyboardSideColumn extends KeyboardColumn {
  readonly mode: string;
  readonly fromCards: boolean;
}

/** A message a row stands for, as the keyboard composition reads it. */
export interface KeyboardMessage {
  readonly thread?: { readonly is_root?: unknown } | null;
}

/** A fold entry, as `isFolded`/`setFolded` read it. */
export interface KeyboardFoldEntry {
  readonly id?: string;
  readonly li: HTMLElement;
}

/** What marking read in the chat service came to, as the wording acts read it. */
export interface KeyboardReadMark {
  readonly service: string;
  readonly asked: boolean;
  readonly marked: boolean;
  readonly already: boolean;
  readonly why: string;
}

/** The page's acts for the keyboard composition: everything the moved region read from the page. */
export interface KeyboardCompositionDependencies<
  Column extends KeyboardColumn = KeyboardColumn,
  Message extends KeyboardMessage = KeyboardMessage,
  ReadMark extends KeyboardReadMark = KeyboardReadMark,
> {
  // Elements, screens and columns.
  readonly screens: Screens;
  el(id: string): HTMLElement;
  leftEl(id: string): HTMLElement;
  documentRoot(): HTMLElement;
  mainColumn(): Column;
  sideColumn(): Column & KeyboardSideColumn;
  inColumn<T>(column: Column, act: () => T): T;
  inSide(): boolean;
  activeColumn(): Column;
  // Pane lists and places.
  shownChannelList(): HTMLElement | null;
  floatingClearance(area: HTMLElement): number;
  channelContextKey(): string;
  selectionView(): string;
  pinsOnly(): boolean;
  threadListShown(): boolean;
  sideColumnUp(): boolean;
  threadListOnScreen(): boolean;
  // Selection and rows.
  rowShown(row: HTMLElement): boolean;
  idsOf(row: HTMLElement): string[];
  shownRowFor(row: HTMLElement): HTMLElement | null;
  rowTimeTabStop(row: HTMLElement, on: boolean): void;
  readerActed(): void;
  putOutLanding(row: HTMLElement): void;
  noteLinkedSelection(): void;
  focusLost(): boolean;
  replyLandingGapPx(): number;
  replyDocked(): boolean;
  replyPane(): KeyPane;
  rowMenuOpen(): boolean;
  openRowMenu(): (SteppedMenu & { close(): void }) | null;
  dwellColumnGlide(pane: KeyPane, row: HTMLElement): void;
  driftFromColumnGlide(): void;
  // Scrolling.
  toFoot(area: HTMLElement): void;
  endFilterWalk(): void;
  closeOverscroll(): void;
  openOverscroll(area: HTMLElement): void;
  scrollToNewest(): void;
  isTextField(node: HTMLElement): boolean;
  scrollsItself(node: HTMLElement): boolean;
  keepsOwnKeys(node: HTMLElement): boolean;
  pressedBySpace(node: HTMLElement): boolean;
  // Moving, Done and folding.
  entryLandingHolds(): boolean;
  entryLandingFirst(): HTMLElement | null;
  setStatus(text: string): void;
  rowIsRead(row: HTMLElement): boolean;
  isDone(id: string): boolean;
  toggleArchived(ids: string[], row: HTMLElement): unknown;
  guardQuietly(run: () => unknown): () => unknown;
  selectedChannelId(): string;
  markRowDone(ids: string[], row: HTMLElement): Promise<unknown>;
  doneSaved(ids: string[]): Promise<boolean>;
  markReadInService(channel: string, id: string): Promise<ReadMark>;
  doneReadWords(mark: ReadMark, also: string): string;
  serviceReadWords(mark: ReadMark, also: string): string;
  localReadWords(wasDone: boolean, also: string): string;
  statusSays(text: string): boolean;
  dismissStatus(): void;
  showingLinks(row: HTMLElement): boolean;
  isFolded(entry: KeyboardFoldEntry): boolean;
  setFolded(entry: KeyboardFoldEntry, folded: boolean): void;
  atBottom(area: HTMLElement): boolean;
  preservingScroll(act: () => void): void;
  renderScrollTools(): void;
  rowMessages(row: HTMLElement): Message[];
  isOpenThreadRoot(row: HTMLElement): boolean;
  viewKey(...args: string[]): string;
  foldedOnes(): Map<string, Set<string>>;
  closeReadRun(key: string, ids: string[]): void;
  renderSideColumn(): void;
  applyReadRuns(): void;
  // Row keys.
  openReadRun(row: HTMLElement): void;
  liveUnpackedRow(row: HTMLElement): HTMLElement | null;
  liveCardRow(row: HTMLElement): HTMLElement | null;
  actOnRow(row: HTMLElement, fold: HTMLElement | null): void;
  cardForArrow(target: HTMLElement): HTMLElement | null;
  columnsShown(): boolean;
  isComposer(node: unknown): boolean;
  closeThread(): unknown;
  sideThreadId(): string;
  cardRowOnRight(id: string): HTMLElement | null;
  closeSideThread(): void;
  toggleMessageDetails(row: HTMLElement, messages: Message[]): void;
  upstreamUrlOf(message: Message): string | null;
  upstreamServiceName(message: Message): string;
  copyUpstreamLink(url: string, service: string): unknown;
  copyDeepLink(message: Message): unknown;
  readCardAloud(card: HTMLElement): void;
  readRowAloud(row: HTMLElement): unknown;
  // Copying.
  combinedContent(messages: readonly Message[]): string;
  renderedHtmlOf(message: Message): string;
  rawTextOf(message: Message): string;
  writeClipboardText(text: string): Promise<boolean>;
  // Views, filters and channels.
  closeSettings(): void;
  pressViewSwitch(): void;
  chooseView(which: "main" | "flat" | "threads" | "columns"): void;
  cycleReadMode(): void;
  readModeSaid(): string;
  searchOpen(): boolean;
  setSearchOpen(open: boolean): void;
  setPinnedOnly(on: boolean): void;
  linksOnly(): boolean;
  setLinksOnly(on: boolean): void;
  cycleAuthorFilter(): void;
  doneOffer(): UndoOfferStamp | null;
  undoDismissal(): unknown;
  sendOffer(): (UndoOfferStamp & { readonly id: string }) | null;
  sendHeld(id: string): boolean;
  putBackDismissedSend(id: string): void;
  bookmarkOffer(): (UndoOfferStamp & { readonly channel: string }) | null;
  currentChannelId(): string;
  undoBookmarkRemoval(): void;
  threadListBack(): void;
  mainRowsOnScreen(): boolean;
  channelsInPickerOrder(): readonly SteppedChannel[];
  chooseChannel(id: string): void;
  changeSelectedChannel(): unknown;
  // Search, Settings, help and Escape.
  showScreen(name: string): void;
  showHelp(entry: string): void;
  openMenus(): Iterable<string>;
  closeOpenMenu(name: string): boolean;
  closeRowMenuOnEscape(event: unknown): boolean;
  replyKeysHere(): boolean;
  closeReply(): void;
  putOutLitCard(): boolean;
  clearAllThreadFocus(): boolean;
  clearLinkedMark(): boolean;
  // The keymap's remaining acts.
  modKeyName(): string;
  manyChatServices(): boolean;
  chatServiceName(): string;
  threadOrderWords(): string;
  minCollapsedLines(): number;
  maxCollapsedLines(): number;
  placeFromKeyboard(step: number): unknown;
  threadStripFromKeyboard(target: HTMLElement): unknown;
  foldStepFromKeyboard(step: number, event: unknown, target: HTMLElement): unknown;
  collapsedLinesFromKeyboard(step: 1 | -1, target: HTMLElement): unknown;
  packFromKeyboard(target: HTMLElement): unknown;
  unpackFromKeyboard(): unknown;
  bookmarkFromKeyboard(target: HTMLElement): unknown;
  bookmarkJumpFromKeyboard(): unknown;
  orderFromKeyboard(): unknown;
  mainRowsFromKeyboard(): unknown;
  columnsFromKeyboard(): unknown;
  // Dispatch, timing and hints.
  childByClass(row: HTMLElement, className: string): HTMLElement | null;
  deviceSettings(): DeviceSettings;
  keyTimingSetting(): boolean;
  locationSearch(): string;
}

/** Everything the page destructures or reads from the keyboard composition, under its old names. */
export interface KeyboardComposition<Column extends KeyboardColumn = KeyboardColumn> {
  readonly channelPane: KeyPane;
  readonly sidePane: KeyPane;
  readonly KEY_PANES: readonly KeyPane[];
  paneColumn(pane: KeyPane): Column;
  readonly lastPane: LastPane<KeyPane>;
  readonly keySelection: KeySelectionModel<KeyPane>;
  readonly paneFor: KeySelectionModel<KeyPane>["paneFor"];
  readonly visibleScroller: KeySelectionModel<KeyPane>["visibleScroller"];
  readonly paneRows: KeySelectionModel<KeyPane>["paneRows"];
  readonly rowOf: KeySelectionModel<KeyPane>["rowOf"];
  readonly paneRowFor: KeySelectionModel<KeyPane>["paneRowFor"];
  readonly markOf: KeySelectionModel<KeyPane>["markOf"];
  readonly markedRow: KeySelectionModel<KeyPane>["markedRow"];
  readonly markNames: KeySelectionModel<KeyPane>["markNames"];
  readonly releaseRow: KeySelectionModel<KeyPane>["releaseRow"];
  readonly activePane: KeySelectionModel<KeyPane>["activePane"];
  readonly drawSelection: KeySelectionModel<KeyPane>["drawSelection"];
  readonly takeRow: KeySelectionModel<KeyPane>["takeRow"];
  readonly noteRow: KeySelectionModel<KeyPane>["noteRow"];
  readonly currentRow: KeySelectionModel<KeyPane>["currentRow"];
  readonly drawnRow: KeySelectionModel<KeyPane>["drawnRow"];
  readonly nearRow: KeySelectionModel<KeyPane>["nearRow"];
  readonly rowInView: KeySelectionModel<KeyPane>["rowInView"];
  readonly firstRowInView: KeySelectionModel<KeyPane>["firstRowInView"];
  readonly selectRow: KeySelectionModel<KeyPane>["selectRow"];
  readonly bringIntoView: KeySelectionModel<KeyPane>["bringIntoView"];
  readonly keepSelection: KeySelectionModel<KeyPane>["keepSelection"];
  readonly inputKinds: InputKinds;
  readonly pointerSelection: PointerSelection<KeyPane>;
  readonly drawnBy: PointerSelection<KeyPane>["drawnBy"];
  readonly keysInUse: PointerSelection<KeyPane>["keysInUse"];
  readonly onPointerMove: PointerSelection<KeyPane>["onPointerMove"];
  readonly onPointerDown: PointerSelection<KeyPane>["onPointerDown"];
  readonly noteClickedRow: PointerSelection<KeyPane>["noteClickedRow"];
  readonly onPaneFocus: PointerSelection<KeyPane>["onPaneFocus"];
  readonly selectLanded: PointerSelection<KeyPane>["selectLanded"];
  readonly followPageScroll: PointerSelection<KeyPane>["followPageScroll"];
  readonly focusList: PointerSelection<KeyPane>["focusList"];
  readonly settleFocus: PointerSelection<KeyPane>["settleFocus"];
  readonly behindThreadList: PointerSelection<KeyPane>["behindThreadList"];
  readonly inColumnGone: PointerSelection<KeyPane>["inColumnGone"];
  readonly scrollKeys: ScrollKeys;
  readonly selectionKeys: SelectionKeys<KeyPane>;
  readonly menuItems: SelectionKeys<KeyPane>["menuItems"];
  readonly stepFromKeyboard: SelectionKeys<KeyPane>["stepFromKeyboard"];
  readonly keyContext: SelectionKeys<KeyPane>["keyContext"];
  readonly rowForAct: SelectionKeys<KeyPane>["rowForAct"];
  readonly doneKeys: DoneKeys;
  readonly nextUnreadFromKeyboard: DoneKeys["nextUnreadFromKeyboard"];
  readonly doneFromKeyboard: DoneKeys["doneFromKeyboard"];
  readonly readCollapse: ReadCollapse<KeyPane>;
  readonly readCollapseFromKeyboard: ReadCollapse<KeyPane>["readCollapseFromKeyboard"];
  readonly foldKeys: FoldKeys<KeyPane>;
  readonly foldForRead: FoldKeys<KeyPane>["foldForRead"];
  readonly foldOneFromKeyboard: FoldKeys<KeyPane>["foldOneFromKeyboard"];
  readonly rowKeys: RowKeys;
  readonly openFromKeyboard: RowKeys["openFromKeyboard"];
  readonly pressRowButton: RowKeys["pressRowButton"];
  readonly threadFromKeyboard: RowKeys["threadFromKeyboard"];
  readonly leaveThread: RowKeys["leaveThread"];
  readonly threadOutFromKeyboard: RowKeys["threadOutFromKeyboard"];
  readonly columnFromKeyboard: RowKeys["columnFromKeyboard"];
  readonly replyFromKeyboard: RowKeys["replyFromKeyboard"];
  readonly menuFromKeyboard: RowKeys["menuFromKeyboard"];
  readonly readFromKeyboard: RowKeys["readFromKeyboard"];
  readonly detailsFromKeyboard: RowKeys["detailsFromKeyboard"];
  readonly copyLinkFromKeyboard: RowKeys["copyLinkFromKeyboard"];
  readonly deepLinkFromKeyboard: RowKeys["deepLinkFromKeyboard"];
  readonly highlightCopy: HighlightCopy;
  readonly COPY_REFUSED: string;
  readonly selectedText: () => string;
  readonly channelOnScreen: () => boolean;
  readonly viewKeys: ViewKeys;
  readonly voiceFromKeyboard: ViewKeys["voiceFromKeyboard"];
  readonly viewFromKeyboard: ViewKeys["viewFromKeyboard"];
  readonly readModeFromKeyboard: ViewKeys["readModeFromKeyboard"];
  readonly filterFromKeyboard: ViewKeys["filterFromKeyboard"];
  readonly undoFromKeyboard: ViewKeys["undoFromKeyboard"];
  readonly writeKeys: WriteKeys;
  readonly composeFromKeyboard: WriteKeys["composeFromKeyboard"];
  readonly writeFromKeyboard: WriteKeys["writeFromKeyboard"];
  readonly newestFromComposer: WriteKeys["newestFromComposer"];
  readonly channelKeys: ChannelKeys;
  readonly stepChannel: ChannelKeys["stepChannel"];
  readonly pickChannel: ChannelKeys["pickChannel"];
  readonly channelPicked: ChannelKeys["channelPicked"];
  readonly screenKeys: ScreenKeys;
  readonly searchFromKeyboard: ScreenKeys["searchFromKeyboard"];
  readonly searchFromAnywhere: ScreenKeys["searchFromAnywhere"];
  readonly settingsFromKeyboard: ScreenKeys["settingsFromKeyboard"];
  readonly closeHelp: ScreenKeys["closeHelp"];
  readonly keysFromKeyboard: ScreenKeys["keysFromKeyboard"];
  readonly replyOnScreen: ScreenKeys["replyOnScreen"];
  readonly onEscape: ScreenKeys["onEscape"];
  readonly KEYMAP: KeymapBinding<HTMLElement>[];
  readonly singleKeys: SingleKeys;
  readonly keyDispatch: KeyDispatch<HTMLElement>;
  readonly keyTimingInAddress: boolean;
  readonly keyTiming: KeyTiming;
  readonly rowShortcuts: RowShortcuts;
  sayRowShortcut(button: Parameters<RowShortcuts["say"]>[0], className: string): void;
  readonly keyHints: KeyHints;
}

/** Build the keyboard composition in the page's order. See the file header for the laziness rule. */
export function createKeyboardComposition<
  Column extends KeyboardColumn = KeyboardColumn,
  Message extends KeyboardMessage = KeyboardMessage,
  FoldEntry extends KeyboardFoldEntry = KeyboardFoldEntry,
  ReadMark extends KeyboardReadMark = KeyboardReadMark,
>(deps: KeyboardCompositionDependencies<Column, Message, ReadMark>): KeyboardComposition<Column> {
  const channelPane: KeyPane = {
    name: "channel",
    scroller: () => deps.leftEl("scroll-area"),
    list: () => deps.inColumn(deps.mainColumn(), () =>
      (deps.screens.screen() === "main" && deps.screens.view() === "discord" && !deps.threadListShown() &&
        (deps.pinsOnly() || deps.selectionView() !== "threads") ? deps.shownChannelList() : null)),
    head: () => deps.floatingClearance(deps.leftEl("scroll-area")),
    foot: () => {
      const area = deps.leftEl("scroll-area");
      const over = ["scroll-tools", "read-steer", "status-line"].reduce((room, id) =>
        room + (deps.leftEl(id).hidden ? 0 : deps.leftEl(id).getBoundingClientRect().height), 0);
      return area.getBoundingClientRect().top + (area.clientHeight || 0) - over;
    },
    context: () => deps.inColumn(deps.mainColumn(), () => JSON.stringify([deps.channelContextKey(), deps.pinsOnly()])),
    marks: new Map(),
    row: null,
    lit: null,
    follow: null,
  };

  const sidePane: KeyPane = {
    name: "side",
    scroller: () => deps.leftEl("side-scroll"),
    list: () => (deps.screens.screen() === "main" && deps.screens.view() === "discord" && deps.sideColumnUp()
      ? deps.leftEl(deps.sideColumn().mode === "thread" ? "side-log" : "side-cards") : null),
    head: () => deps.leftEl("side-scroll").getBoundingClientRect().top,
    foot: () => {
      const area = deps.leftEl("side-scroll");
      const jump = deps.leftEl("side-jump-newest");
      const over = jump.hidden ? 0 : deps.leftEl("side-tools").getBoundingClientRect().height;
      return area.getBoundingClientRect().top + (area.clientHeight || 0) - over;
    },
    context: () => deps.inColumn(deps.sideColumn(), () => JSON.stringify([deps.channelContextKey(), deps.sideColumn().mode])),
    marks: new Map(),
    row: null,
    lit: null,
    follow: null,
  };

  const KEY_PANES: readonly KeyPane[] = [channelPane, sidePane];
  const paneColumn = (pane: KeyPane): Column => (pane === sidePane ? deps.sideColumn() : deps.mainColumn());
  const lastPane = createLastPane(channelPane);

  const keySelection = createKeySelection<KeyPane>({
    keyPanes: () => KEY_PANES,
    channelPane: () => channelPane,
    sidePane: () => sidePane,
    lastPane: () => lastPane.current(),
    takeLastPane: (pane) => lastPane.take(pane),
    threadListOnScreen: () => deps.threadListOnScreen(),
    sideColumnUp: () => deps.sideColumnUp(),
    replyDocked: () => deps.replyDocked(),
    replyPane: () => deps.replyPane(),
    screen: () => deps.screens.screen(),
    byId: (id) => deps.el(id),
    leftById: (id) => deps.leftEl(id),
    rowShown: (row) => deps.rowShown(row),
    idsOf: (row) => deps.idsOf(row),
    shownRowFor: (row) => deps.shownRowFor(row),
    rowTimeTabStop: (row, on) => deps.rowTimeTabStop(row, on),
    readerActedIn: (pane) => deps.inColumn(paneColumn(pane), () => deps.readerActed()),
    putOutLanding: (row) => deps.putOutLanding(row),
    noteLinkedSelection: () => deps.noteLinkedSelection(),
    focusLost: () => deps.focusLost(),
    landingGap: () => deps.replyLandingGapPx(),
  });
  const {
    paneFor, visibleScroller, paneRows, rowOf, paneRowFor, markOf, markedRow, markNames, releaseRow, activePane,
    drawSelection, takeRow, noteRow, currentRow, drawnRow, nearRow, rowInView, firstRowInView, selectRow, bringIntoView,
    keepSelection,
  } = keySelection;

  const inputKinds = createInputKinds({ root: () => deps.documentRoot() });

  const pointerSelection = createPointerSelection<KeyPane, Column, KeySelection>({
    inputKinds,
    screen: () => deps.screens.screen(),
    view: () => deps.screens.view(),
    focusLost: () => deps.focusLost(),
    keyPanes: () => KEY_PANES,
    paneFor: (node) => paneFor(node),
    channelPane: () => channelPane,
    sidePane: () => sidePane,
    lastPane: () => lastPane.current(),
    takeLastPane: (pane) => lastPane.take(pane),
    rowMenuOpen: () => deps.rowMenuOpen(),
    takeRow: (pane, row, focus) => takeRow(pane, row, focus),
    noteRow: (pane, row) => noteRow(pane, row),
    drawSelection: () => drawSelection(),
    rowOf: (pane, node) => rowOf(pane, node),
    rowShown: (row) => deps.rowShown(row),
    markOf: (pane) => markOf(pane),
    markNames: (mark, row) => markNames(mark, row),
    dwellColumnGlide: (pane, row) => deps.dwellColumnGlide(pane, row),
    driftFromColumnGlide: () => deps.driftFromColumnGlide(),
    byId: (id) => deps.el(id),
    leftById: (id) => deps.leftEl(id),
    inSide: () => deps.inSide(),
    mainColumn: () => deps.mainColumn(),
    paneColumn: (pane) => paneColumn(pane),
    inColumn: (column, act) => deps.inColumn(column, act),
    readerActed: () => deps.readerActed(),
    rowInView: (pane, row) => rowInView(pane, row),
    firstRowInView: (pane) => firstRowInView(pane),
    selectRow: (pane, row, bring) => selectRow(pane, row, bring),
    drawnRow: (pane) => drawnRow(pane),
    releaseRow: (pane, row) => releaseRow(pane, row),
    rowTimeTabStop: (row, on) => deps.rowTimeTabStop(row, on),
    visibleScroller: () => visibleScroller(),
    threadListOnScreen: () => deps.threadListOnScreen(),
    sideColumnUp: () => deps.sideColumnUp(),
  });
  const {
    drawnBy, keysInUse, onPointerMove, onPointerDown, noteClickedRow, onPaneFocus, selectLanded, followPageScroll,
    focusList, settleFocus, behindThreadList, inColumnGone,
  } = pointerSelection;

  const scrollKeys = createScrollKeys<KeyPane>({
    scrollArea: () => deps.el("scroll-area"),
    sideScroll: () => deps.leftEl("side-scroll"),
    toFoot: (area) => deps.toFoot(area),
    endFilterWalk: () => deps.endFilterWalk(),
    closeOverscroll: () => deps.closeOverscroll(),
    openOverscroll: (area) => deps.openOverscroll(area),
    scrollToNewest: () => deps.scrollToNewest(),
    visibleScroller: (node) => visibleScroller(node),
    isTextField: (node) => deps.isTextField(node),
    scrollsItself: (node) => deps.scrollsItself(node),
    keepsOwnKeys: (node) => deps.keepsOwnKeys(node),
    pressedBySpace: (node) => deps.pressedBySpace(node),
    onMain: () => deps.screens.screen() === "main",
    paneFor: (node) => paneFor(node),
    paneRows: (pane) => paneRows(pane),
    selectRow: (pane, row, bring) => selectRow(pane, row, bring),
    followPageScroll: (pane, row, up) => followPageScroll(pane, row, up),
  });

  const selectionKeys = createSelectionKeys<KeyPane>({
    paneFor: (node) => paneFor(node),
    paneRows: (pane) => paneRows(pane),
    currentRow: (pane, node) => currentRow(pane, node),
    rowInView: (pane, row) => rowInView(pane, row),
    firstRowInView: (pane) => firstRowInView(pane),
    landingHolds: () => deps.entryLandingHolds(),
    landingFirst: () => deps.entryLandingFirst(),
    selectRow: (...args) => selectRow(...args),
    takeRow: (pane, row, focus) => takeRow(pane, row, focus),
    landingGap: () => deps.replyLandingGapPx(),
    openOverscroll: (area) => deps.openOverscroll(area),
    openRowMenu: () => deps.openRowMenu(),
    setStatus: (text) => deps.setStatus(text),
  });
  const { menuItems, stepFromKeyboard, keyContext, rowForAct } = selectionKeys;

  const doneKeys = createDoneKeys<KeyPane>({
    keyContext: (target) => keyContext(target),
    rowForAct: (target, runLine, stay) => rowForAct(target, runLine, stay),
    paneRows: (pane) => paneRows(pane),
    rowInView: (pane, row) => rowInView(pane, row),
    rowIsRead: (row) => deps.rowIsRead(row),
    selectRow: (pane, row) => selectRow(pane, row),
    setStatus: (text) => deps.setStatus(text),
    paneRowFor: (pane, id, card) => paneRowFor(pane, id, card),
    nearRow: (pane, at) => nearRow(pane, at),
    takeRow: (pane, row, focus) => takeRow(pane, row, focus),
    bringIntoView: (pane, row) => bringIntoView(pane, row),
    idsOf: (row) => deps.idsOf(row),
    isDone: (id) => deps.isDone(id),
    toggleDone: (ids, row) => deps.guardQuietly(() => deps.toggleArchived(ids, row))(),
  });
  const { nextUnreadFromKeyboard, doneFromKeyboard } = doneKeys;

  const readCollapse = createReadCollapse<KeyPane, ReadMark>({
    rowForAct: (target) => rowForAct(target),
    idsOf: (row) => deps.idsOf(row),
    runQuietly: (act) => deps.guardQuietly(act)(),
    channelId: () => deps.selectedChannelId(),
    foldForRead: (pane, row) => foldForRead(pane, row),
    isDone: (id) => deps.isDone(id),
    markRowDone: (ids, row) => deps.markRowDone(ids, row),
    doneSaved: (ids) => deps.doneSaved(ids),
    markReadInService: (channel, id) => deps.markReadInService(channel, id),
    doneReadWords: (mark, also) => deps.doneReadWords(mark, also),
    serviceReadWords: (mark, also) => deps.serviceReadWords(mark, also),
    localReadWords: (wasDone, also) => deps.localReadWords(wasDone, also),
    setStatus: (text) => deps.setStatus(text),
    statusSays: (text) => deps.statusSays(text),
    dismissStatus: () => deps.dismissStatus(),
  });
  const { readCollapseFromKeyboard } = readCollapse;

  const foldKeys = createFoldKeys<KeyPane, FoldEntry, Message, Column>({
    showingLinks: (row) => deps.showingLinks(row),
    isFolded: (entry) => deps.isFolded(entry),
    setFolded: (entry, folded) => deps.setFolded(entry, folded),
    atBottom: (area) => deps.atBottom(area),
    preservingScroll: (act) => deps.preservingScroll(act),
    renderScrollTools: () => deps.renderScrollTools(),
    rowForAct: (target) => rowForAct(target),
    paneRows: (pane) => paneRows(pane),
    shownRowFor: (row) => deps.shownRowFor(row),
    rowMessages: (row) => deps.rowMessages(row),
    isThreadRoot: (message) => Boolean(message.thread && message.thread.is_root),
    isOpenThreadRoot: (row) => deps.isOpenThreadRoot(row),
    setStatus: (text) => deps.setStatus(text),
    idsOf: (row) => deps.idsOf(row),
    threadViewKey: (id) => deps.viewKey("thread", id),
    columnViewKey: (column) => deps.inColumn(column, () => deps.viewKey()),
    paneColumn: (pane) => paneColumn(pane),
    inColumn: (column, act) => deps.inColumn(column, act),
    foldedOnes: () => deps.foldedOnes(),
    closeReadRun: (key, ids) => deps.closeReadRun(key, ids),
    isDone: (id) => deps.isDone(id),
    markDone: (ids, row) => deps.guardQuietly(() => deps.markRowDone(ids, row))(),
    renderSideColumn: () => deps.renderSideColumn(),
    applyReadRuns: () => deps.applyReadRuns(),
    paneRowFor: (pane, id, card) => paneRowFor(pane, id, card),
    selectRow: (pane, row) => selectRow(pane, row),
  });
  const { foldForRead, foldOneFromKeyboard } = foldKeys;

  const rowKeys = createRowKeys<KeyPane, Message>({
    rowForAct: (target, runLine) => rowForAct(target, runLine),
    keyContext: (target) => keyContext(target),
    openReadRun: (row) => deps.openReadRun(row),
    liveUnpackedRow: (row) => deps.liveUnpackedRow(row),
    liveCardRow: (row) => deps.liveCardRow(row),
    selectRow: (...args) => selectRow(...args),
    channelPane: () => channelPane,
    sidePane: () => sidePane,
    focusList: (pane) => focusList(pane),
    settleFocus: () => settleFocus(),
    actOnRow: (row, fold) => deps.actOnRow(row, fold),
    cardForArrow: (target) => deps.cardForArrow(target),
    columnsShown: () => deps.columnsShown(),
    isComposer: (node) => deps.isComposer(node),
    closeThread: () => deps.guardQuietly(() => deps.closeThread())(),
    inSide: () => deps.inSide(),
    screen: () => deps.screens.screen(),
    view: () => deps.screens.view(),
    sideMode: () => deps.sideColumn().mode,
    sideFromCards: () => deps.sideColumn().fromCards,
    sideThreadId: () => deps.sideThreadId(),
    cardRowOnRight: (id) => deps.cardRowOnRight(id),
    closeSideThread: () => deps.closeSideThread(),
    rowShown: (row) => deps.rowShown(row),
    threadListOnScreen: () => deps.threadListOnScreen(),
    selectionView: () => deps.selectionView(),
    channelOnScreen: () => channelOnScreen(),
    replyOnScreen: () => replyOnScreen(),
    byId: (id) => deps.el(id),
    menuItems: () => menuItems(),
    readCardAloud: (card) => deps.readCardAloud(card),
    readRowAloud: (row) => deps.guardQuietly(() => deps.readRowAloud(row))(),
    toggleDetails: (row) => deps.toggleMessageDetails(row, deps.rowMessages(row)),
    firstMessage: (row) => deps.rowMessages(row)[0] as Message,
    upstreamUrlOf: (message) => deps.upstreamUrlOf(message),
    openRowMenu: () => deps.openRowMenu(),
    copyUpstreamLink: (url, message) => deps.guardQuietly(() => deps.copyUpstreamLink(url, deps.upstreamServiceName(message)))(),
    copyDeepLink: (message) => deps.guardQuietly(() => deps.copyDeepLink(message))(),
  });
  const {
    openFromKeyboard, pressRowButton, threadFromKeyboard, leaveThread, threadOutFromKeyboard, columnFromKeyboard,
    replyFromKeyboard, menuFromKeyboard, readFromKeyboard, detailsFromKeyboard, copyLinkFromKeyboard, deepLinkFromKeyboard,
  } = rowKeys;

  const selectedText = (): string =>
    (typeof getSelection === "function" ? String(getSelection() || "") : "");
  const COPY_REFUSED = "This browser would not copy to the clipboard.";

  const highlightCopy = createHighlightCopy<Message>({
    isTextField: (node) => deps.isTextField(node as HTMLElement),
    selectedText: () => selectedText(),
    activePane: () => activePane(),
    rowShown: (row) => deps.rowShown(row),
    rowMessages: (row) => deps.rowMessages(row),
    combinedContent: (messages) => deps.combinedContent(messages),
    renderedHtmlOf: (message) => deps.renderedHtmlOf(message),
    rawTextOf: (message) => deps.rawTextOf(message),
    setStatus: (text) => deps.setStatus(text),
    copyRefused: COPY_REFUSED,
    writeClipboardText: (text) => deps.writeClipboardText(text),
    guardQuietly: (run) => deps.guardQuietly(run),
    setTimeout: (run, ms) => setTimeout(run, ms),
  });

  const channelOnScreen = (): boolean => deps.screens.screen() === "main" && deps.screens.view() === "discord";

  const viewKeys = createViewKeys<Column>({
    screens: deps.screens,
    inSide: () => deps.inSide(),
    mainColumn: () => deps.mainColumn(),
    inColumn: (column, act) => deps.inColumn(column, act),
    closeSettings: () => deps.closeSettings(),
    pressViewSwitch: () => deps.pressViewSwitch(),
    selectedChannelId: () => deps.selectedChannelId(),
    channelOnScreen: () => channelOnScreen(),
    chooseView: (which) => deps.chooseView(which),
    settleFocus: () => settleFocus(),
    readModeHidden: () => deps.el("todo-filter").hidden,
    cycleReadMode: () => deps.cycleReadMode(),
    readModeSaid: () => deps.readModeSaid(),
    setStatus: (text) => deps.setStatus(text),
    threadListOnScreen: () => deps.threadListOnScreen(),
    searchOpen: () => deps.searchOpen(),
    setSearchOpen: (open) => deps.setSearchOpen(open),
    pinnedOnly: () => deps.pinsOnly(),
    setPinnedOnly: (on) => deps.setPinnedOnly(on),
    linksOnly: () => deps.linksOnly(),
    setLinksOnly: (on) => deps.setLinksOnly(on),
    cycleAuthorFilter: () => deps.cycleAuthorFilter(),
    focusChannelList: () => focusList(channelPane),
    doneOffer: () => deps.doneOffer(),
    undoDone: () => deps.guardQuietly(() => deps.undoDismissal())(),
    sendOffer: () => deps.sendOffer(),
    sendHeld: (id) => deps.sendHeld(id),
    putBackSend: (id) => deps.putBackDismissedSend(id),
    bookmarkOffer: () => deps.bookmarkOffer(),
    currentChannelId: () => deps.currentChannelId(),
    undoBookmarkRemoval: () => deps.undoBookmarkRemoval(),
  });
  const { voiceFromKeyboard, viewFromKeyboard, readModeFromKeyboard, filterFromKeyboard, undoFromKeyboard } = viewKeys;

  const writeKeys = createWriteKeys<KeyPane, Column>({
    byId: (id) => deps.el(id),
    leftById: (id) => deps.leftEl(id),
    screen: () => deps.screens.screen(),
    view: () => deps.screens.view(),
    closeSettings: () => deps.closeSettings(),
    replyOnScreen: () => replyOnScreen(),
    channelOnScreen: () => channelOnScreen(),
    threadListOnScreen: () => deps.threadListOnScreen(),
    threadListBack: () => deps.threadListBack(),
    mainRowsOnScreen: () => deps.mainRowsOnScreen(),
    behindThreadList: (node) => behindThreadList(node),
    inColumnGone: (node) => inColumnGone(node),
    isComposer: (node) => deps.isComposer(node),
    inSide: () => deps.inSide(),
    activeColumn: () => deps.activeColumn(),
    mainColumn: () => deps.mainColumn(),
    inColumn: (column, act) => deps.inColumn(column, act),
    channelPane: () => channelPane,
    sidePane: () => sidePane,
    paneFor: (node) => paneFor(node),
    paneColumn: (pane) => paneColumn(pane),
    paneRows: (pane) => paneRows(pane),
    selectRow: (pane, row) => selectRow(pane, row),
  });
  const { composeFromKeyboard, writeFromKeyboard, newestFromComposer } = writeKeys;

  const channelKeys = createChannelKeys({
    channelOnScreen: () => channelOnScreen(),
    picker: () => deps.el("discord-channel") as HTMLElement & ChannelPicker,
    channelsInPickerOrder: () => deps.channelsInPickerOrder(),
    selectedChannelId: () => deps.selectedChannelId(),
    chooseChannel: (id) => deps.chooseChannel(id),
    changeChannel: () => deps.guardQuietly(() => deps.changeSelectedChannel())(),
    settleFocus: () => settleFocus(),
  });
  const { stepChannel, pickChannel, channelPicked } = channelKeys;

  const screenKeys = createScreenKeys<KeyPane>({
    screens: deps.screens,
    showScreen: (name) => deps.showScreen(name),
    closeSettings: () => deps.closeSettings(),
    showHelp: (entry) => deps.showHelp(entry),
    byId: (id) => deps.el(id),
    searchOpen: () => deps.searchOpen(),
    setSearchOpen: (open) => deps.setSearchOpen(open),
    openMenus: () => deps.openMenus(),
    closeOpenMenu: (name) => deps.closeOpenMenu(name),
    closeRowMenuOnEscape: (event) => deps.closeRowMenuOnEscape(event),
    replyDocked: () => deps.replyDocked(),
    replyKeysHere: () => deps.replyKeysHere(),
    closeReply: () => deps.closeReply(),
    threadListOnScreen: () => deps.threadListOnScreen(),
    threadListBack: () => deps.threadListBack(),
    channelOnScreen: () => channelOnScreen(),
    selectionView: () => deps.selectionView(),
    sideShowsCards: () => deps.inSide() && deps.sideColumn().mode === "list",
    leaveThread: () => leaveThread(),
    keyContext: (target) => keyContext(target),
    selectRow: (pane, row, bring) => selectRow(pane, row, bring),
    settleFocus: () => settleFocus(),
    isComposer: (node) => deps.isComposer(node),
    focusList: (pane) => focusList(pane),
    paneFor: (node) => paneFor(node),
    forgetChannelPick: () => channelKeys.forgetPick(),
    putOutLitCard: () => deps.putOutLitCard(),
    clearAllThreadFocus: () => deps.clearAllThreadFocus(),
    clearLinkedMark: () => deps.clearLinkedMark(),
  });
  const { searchFromKeyboard, searchFromAnywhere, settingsFromKeyboard, closeHelp, keysFromKeyboard, replyOnScreen, onEscape } = screenKeys;

  const KEYMAP = createKeymap<HTMLElement>({
    modKeyName: () => deps.modKeyName(),
    manyChatServices: () => deps.manyChatServices(),
    chatServiceName: () => deps.chatServiceName(),
    threadOrderWords: () => deps.threadOrderWords(),
    minCollapsedLines: () => deps.minCollapsedLines(),
    maxCollapsedLines: () => deps.maxCollapsedLines(),
    keyContext: (target) => keyContext(target),
    pressRowButton: (target, className) => pressRowButton(target, className),
    onEscape: (event, target) => onEscape(event, target),
    placeFromKeyboard: (step) => deps.placeFromKeyboard(step),
    listEdge: (target, end) => scrollKeys.listEdge(target, end),
    jumpList: (target, end) => scrollKeys.jumpList(target, end),
    searchFromAnywhere: () => searchFromAnywhere(),
    searchFromKeyboard: () => searchFromKeyboard(),
    settingsFromKeyboard: () => settingsFromKeyboard(),
    keysFromKeyboard: () => keysFromKeyboard(),
    stepFromKeyboard: (...args) => stepFromKeyboard(...args),
    stepChannel: (event) => stepChannel(event),
    pickChannel: () => pickChannel(),
    openFromKeyboard: (event, target) => openFromKeyboard(event, target),
    threadFromKeyboard: (target) => threadFromKeyboard(target),
    threadOutFromKeyboard: () => threadOutFromKeyboard(),
    threadStripFromKeyboard: (target) => deps.threadStripFromKeyboard(target),
    columnFromKeyboard: (right) => columnFromKeyboard(right),
    replyFromKeyboard: (target) => replyFromKeyboard(target),
    menuFromKeyboard: (target) => menuFromKeyboard(target),
    readFromKeyboard: (target) => readFromKeyboard(target),
    detailsFromKeyboard: (target) => detailsFromKeyboard(target),
    copyLinkFromKeyboard: (target) => copyLinkFromKeyboard(target),
    deepLinkFromKeyboard: (target) => deepLinkFromKeyboard(target),
    standalone: () => typeof window.getComputedStyle === "function" &&
      window.getComputedStyle(document.documentElement).getPropertyValue("--app-standalone").trim() === "1",
    copyChordFromKeyboard: () => highlightCopy.copyChordFromKeyboard(),
    copyFromKeyboard: () => highlightCopy.copyFromKeyboard(),
    nextUnreadFromKeyboard: (target) => nextUnreadFromKeyboard(target),
    doneFromKeyboard: (target) => doneFromKeyboard(target),
    readCollapseFromKeyboard: (target) => readCollapseFromKeyboard(target),
    foldOneFromKeyboard: (target) => foldOneFromKeyboard(target),
    foldStepFromKeyboard: (step, event, target) => deps.foldStepFromKeyboard(step, event, target),
    collapsedLinesFromKeyboard: (step, target) => deps.collapsedLinesFromKeyboard(step, target),
    packFromKeyboard: (target) => deps.packFromKeyboard(target),
    unpackFromKeyboard: () => deps.unpackFromKeyboard(),
    bookmarkFromKeyboard: (target) => deps.bookmarkFromKeyboard(target),
    bookmarkJumpFromKeyboard: () => deps.bookmarkJumpFromKeyboard(),
    viewFromKeyboard: (view, repeat) => viewFromKeyboard(view, repeat ?? false),
    voiceFromKeyboard: (repeat) => voiceFromKeyboard(repeat ?? false),
    readModeFromKeyboard: () => readModeFromKeyboard(),
    filterFromKeyboard: (name) => filterFromKeyboard(name),
    undoFromKeyboard: () => undoFromKeyboard(),
    orderFromKeyboard: () => deps.orderFromKeyboard(),
    mainRowsFromKeyboard: () => deps.mainRowsFromKeyboard(),
    columnsFromKeyboard: () => deps.columnsFromKeyboard(),
    composeFromKeyboard: () => composeFromKeyboard(),
    writeFromKeyboard: () => writeFromKeyboard(),
    newestFromComposer: (target) => newestFromComposer(target),
  });

  const singleKeys = createSingleKeys({
    deviceSettings: () => deps.deviceSettings(),
    render: () => keyHints.render(),
  });

  const keyDispatch = createKeyDispatch<HTMLElement, KeyPane, Column>({
    keymap: () => KEYMAP,
    singleKeysOn: () => singleKeys.current(),
    isTextField: (target) => deps.isTextField(target),
    keepsOwnKeys: (target) => deps.keepsOwnKeys(target),
    keysInUse: () => keysInUse(),
    modKeyName: () => deps.modKeyName(),
    activeElement: () => document.activeElement as HTMLElement,
    onMain: () => deps.screens.screen() === "main",
    paneFor: (target) => paneFor(target),
    paneColumn: (pane) => paneColumn(pane),
    mainColumn: () => deps.mainColumn(),
    inColumn: (column, act) => deps.inColumn(column, act),
    listGroups: [KEYS_MOVING, KEYS_MESSAGE],
    readerActed: () => deps.readerActed(),
    routeScrollKey: (event, target) => scrollKeys.routeScrollKey(event, target),
  });

  const keyTimingInAddress = /[?&]key-timing(=|&|$)/.test(deps.locationSearch() || "");

  const keyTiming = createKeyTiming({
    enabled: () => keyTimingInAddress || deps.keyTimingSetting(),
    now: () => performance.now(),
    afterPaint: (act) => requestAnimationFrame(() => setTimeout(act, 0)),
    report: (entry, start) => {
      console.info(`[key-timing] ${keyTimingLine(entry)}`);
      try {
        performance.measure(`key ${entry.chord}`, { start, duration: entry.paint || 0 });
      } catch (_error) {
        // A browser without user timing still has the console line.
      }
    },
  });

  const rowShortcuts = createRowShortcuts();
  const sayRowShortcut = (button: Parameters<RowShortcuts["say"]>[0], className: string): void => {
    rowShortcuts.say(button, className);
  };

  const keyHints = createKeyHints({
    keymap: () => KEYMAP,
    singleKeysOn: () => singleKeys.current(),
    modKeyName: () => deps.modKeyName(),
    byId: (id) => deps.el(id),
    rowShortcuts,
    childByClass: (row, className) => deps.childByClass(row, className),
    groups: [KEYS_PAGE, KEYS_MOVING, KEYS_MESSAGE, KEYS_VIEW, KEYS_WRITING],
    viewGroup: KEYS_VIEW,
  });

  return {
    channelPane, sidePane, KEY_PANES, paneColumn, lastPane, keySelection,
    paneFor, visibleScroller, paneRows, rowOf, paneRowFor, markOf, markedRow, markNames, releaseRow, activePane,
    drawSelection, takeRow, noteRow, currentRow, drawnRow, nearRow, rowInView, firstRowInView, selectRow, bringIntoView,
    keepSelection, inputKinds, pointerSelection,
    drawnBy, keysInUse, onPointerMove, onPointerDown, noteClickedRow, onPaneFocus, selectLanded, followPageScroll,
    focusList, settleFocus, behindThreadList, inColumnGone, scrollKeys, selectionKeys,
    menuItems, stepFromKeyboard, keyContext, rowForAct, doneKeys, nextUnreadFromKeyboard, doneFromKeyboard,
    readCollapse, readCollapseFromKeyboard, foldKeys, foldForRead, foldOneFromKeyboard, rowKeys,
    openFromKeyboard, pressRowButton, threadFromKeyboard, leaveThread, threadOutFromKeyboard, columnFromKeyboard,
    replyFromKeyboard, menuFromKeyboard, readFromKeyboard, detailsFromKeyboard, copyLinkFromKeyboard, deepLinkFromKeyboard,
    highlightCopy, COPY_REFUSED, selectedText, channelOnScreen, viewKeys,
    voiceFromKeyboard, viewFromKeyboard, readModeFromKeyboard, filterFromKeyboard, undoFromKeyboard,
    writeKeys, composeFromKeyboard, writeFromKeyboard, newestFromComposer, channelKeys, stepChannel, pickChannel, channelPicked,
    screenKeys, searchFromKeyboard, searchFromAnywhere, settingsFromKeyboard, closeHelp, keysFromKeyboard, replyOnScreen, onEscape,
    KEYMAP, singleKeys, keyDispatch, keyTimingInAddress, keyTiming, rowShortcuts, sayRowShortcut, keyHints,
  };
}

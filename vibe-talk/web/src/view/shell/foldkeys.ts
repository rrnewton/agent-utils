// Folding a message from the keyboard (`#295 shift-e-read-collapse`, `#480 fold-one-message-key`, `#280
// scroll-stability`; `#624 voice-page-modularity`, moved from the legacy script into view/shell for the plan's
// step 17): Shift+O folds the selected message away into a line of read messages and steps above it, and Shift+E's
// fold keeps what the reader is looking at where it was. Marking read and Done, the folds and runs, and drawing
// the lists are the page's.

/** A list the keys move through, as far as folding reads it. */
export interface FoldedPane {
  /** The list's scroller. */
  scroller(): HTMLElement;
  /** Where the list's view begins and ends, under the bars over it and above the tools at its foot. */
  head(): number;
  foot(): number;
}

/** A row with the fold it may have. */
export type FoldableRow<Entry> = HTMLElement & { readonly foldEntry?: Entry | null };

export interface FoldKeysDependencies<Pane extends FoldedPane, Entry, Message, Column> {
  /** Whether `row` shows its links in place of its text. */
  showingLinks(row: HTMLElement): boolean;
  /** Whether the fold `entry` is folded, and folding or unfolding it. */
  isFolded(entry: Entry): boolean;
  setFolded(entry: Entry, folded: boolean): void;
  /**
   * Collapse `row` in `area` under the scroll owner's per-act anchor choice (`#1251
   * scroll-single-owner`, `#1215 collapse-anchor-below`): the owner holds the row, the next
   * row, the foot or the row's head, by where the row stands.
   */
  preservingCollapse(area: HTMLElement, row: HTMLElement, head: number, foot: number, act: () => void): void;
  /** Draw the chips over the lists' feet again. */
  renderScrollTools(): void;
  /** The row a key acts on from `target`, and whether the key was taken even with no row to act on. */
  rowForAct(target: HTMLElement): { readonly pane: Pane | null; readonly row: HTMLElement | null; readonly took: boolean };
  /** The rows of `pane`'s list. */
  paneRows(pane: Pane): HTMLElement[];
  /** The row a member of a folded run is shown as. */
  shownRowFor(row: HTMLElement): HTMLElement | null;
  /** The messages `row` stands for. */
  rowMessages(row: HTMLElement): Message[];
  /** Whether `message` is its thread's first. */
  isThreadRoot(message: Message): boolean;
  /** Whether `row` is the first message of the thread open in the column the code is working in. */
  isOpenThreadRoot(row: HTMLElement): boolean;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
  /** The message ids `row` stands for. */
  idsOf(row: HTMLElement): string[];
  /** The key of the view of the thread `id`, and of the view on screen in `column`. */
  threadViewKey(id: string): string;
  columnViewKey(column: Column): string;
  /** The column of `pane`. */
  paneColumn(pane: Pane): Column;
  /** Run `act` in `column`. */
  inColumn(column: Column, act: () => void): void;
  /** The messages folded away by Shift+O, by view key. */
  foldedOnes(): Map<string, Set<string>>;
  /** Close the read run of `ids` in the view `key`. */
  closeReadRun(key: string, ids: string[]): void;
  /** Whether the message `id` is Done, and marking the messages `ids` of `row` Done (a failure reported without ending a call). */
  isDone(id: string): boolean;
  markDone(ids: string[], row: HTMLElement): void;
  /** Draw the right column again, and the read runs of the column the code is working in. */
  renderSideColumn(): void;
  applyReadRuns(): void;
  /** The row of `pane` for the message `id`, or null. */
  paneRowFor(pane: Pane, id: string, card: boolean): HTMLElement | null;
  /** Select `row` of `pane`, brought into view. */
  selectRow(pane: Pane, row: HTMLElement): void;
}

/** Folding from the keyboard. */
export interface FoldKeys<Pane> {
  /**
   * Fold `row` for Shift+E with what the reader is looking at left where it was (`#280
   * scroll-stability`), and answer whether it is folded now. A short message has no fold, and a row
   * showing its links in place of its text is not folded out of sight (`actOnRow`).
   *
   * The anchor is the scroll owner's per-act choice (`#1251 scroll-single-owner`, `#1215
   * collapse-anchor-below`): the row itself while its head is on screen, the next row when its
   * head is already above the viewport, its head brought to the top when it is taller than the
   * screen, and the foot for a reader on the newest line.
   */
  foldForRead(pane: Pane, row: HTMLElement): boolean;
  /**
   * Shift+O: fold the selected message away into a "… read messages …" line, and step to the
   * message above (`#480 fold-one-message-key`). The owner's way through one thread at a time:
   * press, press, press, closing up what is dealt with, until the thread's first message — which
   * stays, and the key does nothing there. Adjacent folded messages are one line with a count, by
   * the same plan as Fold read's runs (`readRunPlan`), in any read mode; Enter or a tap on the
   * line opens it again (`openReadRun` takes its ids back out of `foldedOnes`).
   *
   * Folding a message away marks it Done when it was not already, as `e` marks it: the line says
   * "read messages", and a message the reader has closed up as dealt with is read. A message
   * already Done is not marked again. On a run's own line the key steps above it and folds that
   * message, so spamming it keeps going up. A card is packed by its own control, not folded here.
   */
  foldOneFromKeyboard(target: HTMLElement): boolean;
}

export function createFoldKeys<Pane extends FoldedPane, Entry, Message, Column>(
  deps: FoldKeysDependencies<Pane, Entry, Message, Column>,
): FoldKeys<Pane> {
  return {
    foldForRead(pane, row) {
      const entry = (row as FoldableRow<Entry>).foldEntry;
      if (!entry || deps.showingLinks(row)) return Boolean(entry && deps.isFolded(entry));
      if (deps.isFolded(entry)) return true;
      deps.preservingCollapse(pane.scroller(), row, pane.head(), pane.foot(), () => deps.setFolded(entry, true));
      deps.renderScrollTools();
      return true;
    },
    foldOneFromKeyboard(target) {
      const { pane, row, took } = deps.rowForAct(target);
      if (!pane || !row) return took;
      if (row.hasAttribute("data-card")) return false;
      let at: HTMLElement | null = row;
      // On a folded line already: the message to fold is the one above it.
      while (at && at.getAttribute("data-read-run")) {
        const rows = deps.paneRows(pane);
        at = rows[rows.indexOf(at) - 1] || null;
        if (at && at.getAttribute("data-read-run") === "member") at = deps.shownRowFor(at);
      }
      if (!at || at.hasAttribute("data-card")) return true;
      const folding: HTMLElement = at;
      const unpack = folding.getAttribute("data-unpack-thread");
      const root = unpack !== null
        ? deps.rowMessages(folding).some((message) => deps.isThreadRoot(message))
        : deps.isOpenThreadRoot(folding);
      if (root) {
        deps.setStatus("The thread's first message stays.");
        return true;
      }
      const ids = deps.idsOf(folding);
      if (ids.length === 0) return false;
      const key = unpack !== null ? deps.threadViewKey(unpack) : deps.columnViewKey(deps.paneColumn(pane));
      const folded = deps.foldedOnes().get(key) || new Set();
      for (const id of ids) folded.add(id);
      deps.foldedOnes().set(key, folded);
      deps.closeReadRun(key, ids);
      if (!ids.every((id) => deps.isDone(id))) deps.markDone(ids, folding);
      if (unpack !== null) {
        deps.renderSideColumn();
      } else {
        deps.inColumn(deps.paneColumn(pane), () => {
          deps.applyReadRuns();
          deps.renderScrollTools();
        });
      }
      // Above the line it folded into, or on the line itself when there is nothing above.
      const landed = deps.paneRowFor(pane, ids[0] as string, false);
      const rows = deps.paneRows(pane);
      const above = landed ? rows[rows.indexOf(landed) - 1] : null;
      deps.selectRow(pane, (above || landed || folding) as HTMLElement);
      deps.setStatus("Folded away.");
      return true;
    },
  };
}

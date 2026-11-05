// Shift+E, read and collapse (`#295 shift-e-read-collapse`, `#300 reply-marks-read`, `#370 shift-e-undo-marker`;
// `#624 voice-page-modularity`, moved from the legacy script into view/shell for the plan's step 17): the selected
// message marked read and folded, and the chat service's own read marker moved through it where it can be. Marking
// Done, the service's marker, folding and the words the status line says are the page's.

/** The row a key acts on: its pane and row, and whether the key was taken even with no row to act on. */
export interface CollapsedRow<Pane> {
  readonly pane: Pane | null;
  readonly row: HTMLElement | null;
  readonly took: boolean;
}

export interface ReadCollapseDependencies<Pane, Mark> {
  /** The row a key acts on from `target`. */
  rowForAct(target: HTMLElement): CollapsedRow<Pane>;
  /** The message ids `row` stands for. */
  idsOf(row: HTMLElement): string[];
  /** Run `act`, a failure said on the error line and ending nothing else. */
  runQuietly(act: () => Promise<void>): void;
  /** The channel on screen. */
  channelId(): string;
  /** Fold `row` of `pane` for Shift+E, the reader's place kept; whether it is folded now. */
  foldForRead(pane: Pane, row: HTMLElement): boolean;
  /** Whether the message `id` is Done. */
  isDone(id: string): boolean;
  /** Mark the messages `ids` of `row` Done; a failure rejects. */
  markRowDone(ids: string[], row: HTMLElement): Promise<unknown>;
  /** Whether the Done of the messages `ids` is saved, waiting while it is on its way; false once it failed or was undone. */
  doneSaved(ids: string[]): Promise<boolean>;
  /** Ask the server to move the chat service's read marker of `channel` through the message `id`. */
  markReadInService(channel: string, id: string): Promise<Mark>;
  /** What the status line says of `mark`, for a message Done already or marked Done now; `also` follows the mark. */
  doneReadWords(mark: Mark, also: string): string;
  serviceReadWords(mark: Mark, also: string): string;
  /** Say `text` on the status line. */
  setStatus(text: string): void;
}

/** Shift+E. */
export interface ReadCollapse<Pane> {
  /**
   * Shift+E: read and collapse (`#295 shift-e-read-collapse`). The owner's idiom for "dealt with, and
   * give me the room back": the selected message marked read — Done, as its Done button and e mark it,
   * and in the chat service too where it can be, as a sent reply marks what it answers — and folded, as
   * its fold control folds it (`readAndCollapse`). In a thread, or in the right column, the message
   * selected there. Only ever a mark: a message already Done stays Done and is folded all the same. The
   * selection stays on the message, and the list does not move under the reader (`foldForRead`). The
   * read modes treat it as any Done: under Fold read a message that joins a run of read messages
   * leaves the selection on the run's line, and under Hide read it goes, the selection to the next row.
   *
   * IT USED TO BE DONE THROUGH HERE (`#202 read-through-here`): every message above as well, which one
   * press by accident turned into a read history lost, and Undo once saved it. That act now has no key
   * at all; it is the ⋯ menu's Through here alone, with its Undo.
   */
  readCollapseFromKeyboard(target: HTMLElement): boolean;
  /**
   * Shift+E's act on `row`: folded first, while it is the row on screen, then marked read. The fold is
   * recorded by message id (`setFolded`), so the row a Done redraws is drawn folded too.
   *
   * Marked read as a sent reply marks what it answers (`#300 reply-marks-read`): Done (`markRowDone`),
   * and once the server has that Done, the chat service's own read marker moved through the row's
   * newest message, where its provider lets the server move it and the marker covers no more than the
   * channel (`markReadInService`) — forward only and asked once, by the server's floor. A Done that
   * fails asks the service nothing: the failure is said, and the row is unread again.
   *
   * A MESSAGE ALREADY DONE is not marked Done again, nor put back: Shift+E is only ever a mark. Its
   * chat service's half is still asked, the same `after_reply` mark, because the Done may have come
   * from `e`, the Done button or a swipe, which leave the service's marker where it was, or from an
   * earlier Shift+E whose service refused it for a while. Where the server has marked that message or a
   * later one already — a reply, Through here, an earlier Shift+E — its floor answers `already_marked`
   * or `behind` and asks the service nothing (src/reply_read.rs), so pressing it again costs the
   * service no read of its marker.
   *
   * DONE HERE IS NOT YET SAVED while its write is on its way: a message is drawn Done the moment it is
   * marked. Shift+E pressed again meanwhile, or after `e` or the Done button, waits for that Done to be
   * saved before it asks the service anything, and asks, and says, nothing once it has failed
   * (`doneSaved`): the service's marker cannot be moved back, and the failure is said by the act it
   * failed for. Its own Done is looked at the same way once saved (`#370 shift-e-undo-marker`, as a
   * sent reply's is since `#327 done-races-remaining`): an Undo pressed while it was on its way, the
   * Undo itself saved, leaves the service unasked for a message put back.
   *
   * The status line says where the message was marked, and that it was collapsed, once the service has
   * answered; nothing over another channel the reader has gone to meanwhile.
   */
  readAndCollapse(pane: Pane, row: HTMLElement, ids: string[]): Promise<void>;
}

export function createReadCollapse<Pane, Mark>(deps: ReadCollapseDependencies<Pane, Mark>): ReadCollapse<Pane> {
  async function readAndCollapse(pane: Pane, row: HTMLElement, ids: string[]): Promise<void> {
    const channel = String(deps.channelId());
    const folded = deps.foldForRead(pane, row);
    const wasDone = ids.every((id) => deps.isDone(id));
    if (!wasDone) await deps.markRowDone(ids, row);
    if (!(await deps.doneSaved(ids))) return;
    const mark = await deps.markReadInService(channel, ids[ids.length - 1] as string);
    if (channel !== String(deps.channelId())) return;
    const also = folded ? ", and collapsed" : "";
    deps.setStatus(wasDone ? deps.doneReadWords(mark, also) : deps.serviceReadWords(mark, also));
  }

  return {
    readCollapseFromKeyboard(target) {
      const { pane, row, took } = deps.rowForAct(target);
      const ids = row ? deps.idsOf(row) : [];
      if (!pane || !row || ids.length === 0) return took;
      deps.runQuietly(() => readAndCollapse(pane, row, ids));
      return true;
    },
    readAndCollapse,
  };
}

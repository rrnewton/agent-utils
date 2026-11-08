// Taking a send back (`#193 compose-draft-loss`, `#277 send-order`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel; mu-readcore):
// Edit puts a failed send's unsent text back in the box it was written in, Dismiss keeps a
// copy the Undo chip and Settings can put back, and the dismissed pop-out is listened to
// here. Dropping an entry from the outbox, and whether one may be taken back, are
// core/outbox dispatch's; the draft stores, boxes and Settings list are the page's, in deps.

import type { Message } from "../../contract/index.ts";

/** A pending send as taking it back reads it (the outbox's live entry shape, mirrored). */
export interface TakeBackEntry {
  id: string;
  channel: string;
  threadId?: string | null;
  replyTo?: string | null;
  text: string;
  remaining?: string;
  state: string;
  phase?: string;
  detail?: string;
  postedCount?: number;
  mayHavePosted?: boolean;
  retriesUsed?: number;
  parts: readonly { readonly id: string }[];
  observedParts: readonly string[];
}

/** A dismissed send's wait for an automatic send, kept for Undo to set waiting again. */
export interface DismissalHold {
  at: number;
  offline: boolean;
  unsent: boolean;
  identity: unknown;
  retriesUsed: number;
}

/** One dismissed send as its store holds it. */
export interface DismissedHeld {
  entry: TakeBackEntry;
  at: number;
}

/** What taking a send back asks of the page. */
export interface SendTakeBackDependencies<Column = unknown> {
  el(id: string): HTMLElement;
  outbox: {
    get(id: string): TakeBackEntry | undefined;
    has(id: string): boolean;
    putBackInOrder(entry: TakeBackEntry): void;
  };
  drop(id: string): void;
  editable(entry: TakeBackEntry): boolean;
  waitingItsTurn(entry: TakeBackEntry): boolean;
  retryOf(id: string): { at: number; offline: boolean; unsent: boolean; identity: unknown } | undefined;
  restoreHold(entry: TakeBackEntry, hold: { at: number; offline: boolean; unsent: boolean }): void;
  identity(): unknown;
  online(): boolean;
  fromSaved(saved: unknown): TakeBackEntry | null;
  unsentText(entry: TakeBackEntry): string;
  underDraft(before: string | undefined, text: string): string;
  snippet(text: string): string;
  selectedChannelId(): string;
  composerKey(): string;
  activeColumn(): Column;
  mainColumn: Column;
  sideColumn: Column;
  sideShowing(): boolean;
  inColumn<T>(column: Column, run: () => T): T;
  channelDrafts: {
    get(key: string): string | undefined;
    set(key: string, text: string): void;
    delete(key: string): void;
  };
  saveChannelDrafts(): boolean;
  noteDraft(kind: string, key: string, channel: string, held: boolean): void;
  openThread(id: string): unknown;
  fitBox(box: HTMLElement): void;
  focusAtEnd(box: HTMLElement): void;
  setStatus(text: string): void;
  replyDrafts: {
    get(id: string): string | undefined;
    set(id: string, text: string): void;
    delete(id: string): void;
    save(): boolean;
    setStartsThread(id: string, starts: boolean): void;
  };
  renderDraftMarkers(): void;
  idsOf(row: Element): string[];
  rowMessages(row: Element): Message[];
  heldMessage(id: string): Message | null | undefined;
  heldThreadRecord(message: Message): { id?: unknown; is_root?: unknown } | null;
  openReply(messages: Message[], row: Element | null): void;
  dismissed: {
    add(entry: TakeBackEntry): boolean;
    find(id: string): DismissedHeld | undefined;
    remove(held: DismissedHeld): boolean;
    clear(): boolean;
    load(): void;
  };
  dismissedView: {
    render(): void;
    close(): void;
    copy(): void;
    restore(): void;
    trapTab(event: KeyboardEvent): void;
  };
  undo: {
    offer(dismissal: { id: string; hold: DismissalHold | null; at: number }): void;
    current(): { id: string; hold: unknown; at: number } | null;
    end(): void;
  };
  nextUndoAt(): number;
  persist(): void;
  renderOutgoing(): void;
  channelLabel(channel: string): string;
  view(): string;
}

/** Taking a send back: the legacy commands, unchanged. */
export interface SendTakeBack {
  edit(id: string): Promise<void>;
  dismiss(id: string): void;
  endUndo(): void;
  renderUndo(): void;
  putBack(id: string): void;
  forget(): void;
  load(): void;
  dispose(): void;
}

/** Said where Edit could not save the text as a draft, and so left the send where it was. */
const EDIT_DRAFT_REFUSED = "This browser could not save a draft, so the message stays in the outbox.";

/** Taking sends back over `deps`; the dismissed pop-out's listeners are mounted with it. */
export function createSendTakeBack<Column = unknown>(deps: SendTakeBackDependencies<Column>): SendTakeBack {
  const el = deps.el;

  /**
   * Edit: a failed send's unsent text back in the box it was written in, and out of the outbox. Written
   * into that box's draft store first, and taken out of the outbox only once the store holds it, so
   * there is no moment at which neither holds it; where the store refuses it, nothing moves.
   */
  async function editOutgoingMessage(id: string): Promise<void> {
    const entry = deps.outbox.get(id);
    if (!entry || !deps.editable(entry)) return;
    const text = deps.unsentText(entry);
    if (!text.trim()) return;
    if (entry.replyTo) {
      editOutgoingReply(entry, text);
      return;
    }
    const key = JSON.stringify([entry.channel, entry.threadId || null]);
    // The column whose box that is now, if one is showing it: this one first, then the other.
    const boxShown = () => String(deps.selectedChannelId()) === entry.channel && deps.composerKey() === key;
    const showing = (): Column | null => [deps.activeColumn(), deps.activeColumn() === deps.mainColumn ? deps.sideColumn : deps.mainColumn]
      .find((column) => (column === deps.mainColumn || deps.sideShowing()) && deps.inColumn(column, boxShown)) || null;
    // From the store, which every keystroke in the box writes, and which holds the draft of a box no
    // column is showing.
    const before = deps.channelDrafts.get(key);
    deps.channelDrafts.set(key, deps.underDraft(before, text));
    if (!deps.saveChannelDrafts()) {
      if (before === undefined) deps.channelDrafts.delete(key);
      else deps.channelDrafts.set(key, before);
      deps.setStatus(EDIT_DRAFT_REFUSED);
      return;
    }
    deps.noteDraft("composer", key, entry.channel, true);
    deps.drop(entry.id);
    let column = showing();
    // A thread's message, edited from All: its box is the thread's, so the thread opens, with it.
    if (!column && entry.threadId) {
      await deps.openThread(entry.threadId);
      column = showing();
    }
    if (!column) {
      deps.setStatus("Your message is back in its message box, as a draft.");
      return;
    }
    deps.inColumn(column, () => {
      const box = el("channel-compose-text") as HTMLTextAreaElement;
      box.value = deps.channelDrafts.get(key) || "";
      deps.fitBox(box);
      el("channel-compose-state").textContent = before && before.trim()
        ? "Your message is back, under the draft that was already here." : "";
      deps.focusAtEnd(box);
    });
  }

  /**
   * Edit, for a reply: the text back in Reply on the message it answers, under a draft already written
   * to it, and the new-thread choice as it was sent. The message is the row's on screen, or the store's;
   * one this page no longer holds keeps the draft for when it is drawn again (`draftMarker`).
   */
  function editOutgoingReply(entry: TakeBackEntry, text: string): void {
    const id = String(entry.replyTo);
    const before = deps.replyDrafts.get(id);
    deps.replyDrafts.set(id, deps.underDraft(before, text));
    if (!deps.replyDrafts.save()) {
      if (before === undefined) deps.replyDrafts.delete(id);
      else deps.replyDrafts.set(id, before);
      deps.setStatus(EDIT_DRAFT_REFUSED);
      return;
    }
    deps.noteDraft("reply", id, entry.channel, true);
    deps.drop(entry.id);
    deps.renderDraftMarkers();
    const row = [...el("discord-log").children].find((li) => deps.idsOf(li)[0] === id) || null;
    const held = deps.heldMessage(id);
    const messages = row ? deps.rowMessages(row) : held ? [held] : [];
    if (messages.length === 0) {
      deps.setStatus("Your reply is a draft again, on the message it answers: Draft on that message opens it.");
      return;
    }
    // `#185 reply-new-thread`. Sent into the thread its message heads, it is bound for that thread again,
    // as a draft kept with its tick is: ticked where the thread is still nobody's, and into it where
    // somebody has answered it since (`replyRoute`).
    const record = deps.heldThreadRecord(messages[0]!);
    if (entry.threadId && record && record.is_root === true && String(record.id) === String(entry.threadId)) {
      deps.replyDrafts.setStartsThread(id, true);
    }
    deps.openReply(messages, row);
    el("reply-state").textContent = before && before.trim()
      ? "Your reply is back, under the draft that was already here." : "";
    deps.focusAtEnd(el("reply-text"));
  }

  /** Dismiss: out of the outbox, into the recently dismissed, with Undo offered for a while. */
  function dismissOutgoingMessage(id: string): void {
    const entry = deps.outbox.get(id);
    if (!entry || (entry.state === "sending" && !deps.waitingItsTurn(entry))) return;
    // `#277 send-order`. One taken back while it waited its turn was not sent this time: kept as that,
    // never as a send that was on its way when the page closed — and, where an earlier try of it may
    // have landed, as unconfirmed rather than as never sent.
    const copy: TakeBackEntry = entry.state !== "sending" ? { ...entry, phase: "" }
      : entry.mayHavePosted
        ? { ...entry, state: "unconfirmed", phase: "",
          detail: "Dismissed while it waited its turn to go again, after a try whose delivery was not confirmed." }
        : { ...entry, state: "failed", phase: "", detail: "Dismissed while it waited its turn, so it was not sent." };
    const kept = deps.dismissed.add(copy);
    // A send waiting to go again on its own, for Undo to set waiting again. Never saved, as the wait
    // itself is not: Put back after a reload, or from Settings later, leaves it to Retry.
    const waiting = deps.retryOf(id);
    const hold: DismissalHold | null = waiting ? { at: waiting.at, offline: waiting.offline, unsent: waiting.unsent,
      identity: waiting.identity, retriesUsed: entry.retriesUsed || 0 } : null;
    deps.drop(id);
    deps.undo.offer({ id, hold, at: deps.nextUndoAt() });
    renderSendUndo();
    deps.dismissedView.render();
    deps.setStatus(kept
      ? "Dismissed. Undo it, or find it in Settings."
      : "Dismissed. This browser could not keep a copy, so Undo works only until this page is reloaded.");
  }

  function endSendUndo(): void {
    deps.undo.end();
  }

  /** The Undo chip: up while a Dismiss can be taken back from it, naming the message dismissed. */
  function renderSendUndo(): void {
    const chip = el("undo-send-dismiss");
    const offer = deps.undo.current();
    const held = offer ? deps.dismissed.find(offer.id) : null;
    chip.hidden = !held || deps.view() !== "discord";
    const label = held ? `Undo dismiss: “${deps.snippet(deps.unsentText(held.entry))}”` : "Undo dismiss";
    if (chip.textContent !== label) chip.textContent = label;
  }

  /**
   * Put a dismissed send back in the outbox: what Undo does, and Put back in Settings. Undo puts back
   * the wait for an automatic send as well, for the credential it was armed under.
   */
  function putBackDismissedSend(id: string): void {
    const held = deps.dismissed.find(id);
    if (!held) return;
    const entry = deps.fromSaved(held.entry);
    if (entry && !deps.outbox.has(entry.id)) {
      // In the order it was written among the sends still there, as it was before it was dismissed.
      deps.outbox.putBackInOrder(entry);
      const offer = deps.undo.current();
      // The wait the dismissal kept, as `dismissOutgoingMessage` made it: its time, offline, unsent, identity and retries.
      const hold = (offer && offer.id === id ? offer.hold : null) as DismissalHold | null;
      if (hold && hold.identity === deps.identity()) {
        entry.retriesUsed = hold.retriesUsed;
        // Waiting for the connection by what the browser says now: one that came back while the send
        // was dismissed fired its `online` with nothing waiting, and will not fire it again.
        deps.restoreHold(entry, { at: hold.at, offline: !deps.online(), unsent: hold.unsent });
      }
    }
    // The outbox holds it again before the copy goes.
    deps.persist();
    deps.dismissed.remove(held);
    const offered = deps.undo.current();
    if (offered && offered.id === id) endSendUndo();
    deps.renderOutgoing();
    deps.dismissedView.render();
    if (!entry) return;
    deps.setStatus(entry.channel === String(deps.selectedChannelId())
      ? `Put back: “${deps.snippet(deps.unsentText(entry))}”.`
      : `Put back in the outbox of ${deps.channelLabel(entry.channel)}.`);
  }

  /** Forget every dismissed send now, rather than when its days are up. */
  function forgetDismissedSends(): void {
    deps.dismissed.clear();
    endSendUndo();
    deps.dismissedView.render();
    deps.setStatus("Forgot the dismissed messages.");
  }

  /** What the device kept of the recently dismissed, pruned; a damaged copy is nothing. */
  function loadDismissedSends(): void {
    deps.dismissed.load();
  }

  // Settings' Recently dismissed, each send's place, age and first line, is drawn by
  // web/src/view/settings (`dismissedView.render`); its pop-out is listened to here.
  const onTrapTab = (event: KeyboardEvent) => deps.dismissedView.trapTab(event);
  const onClose = () => deps.dismissedView.close();
  const onCopy = () => deps.dismissedView.copy();
  const onRestore = () => deps.dismissedView.restore();
  document.addEventListener("keydown", onTrapTab);
  el("dismissed-send-close").addEventListener("click", onClose);
  el("dismissed-send-copy").addEventListener("click", onCopy);
  el("dismissed-send-restore").addEventListener("click", onRestore);

  return {
    edit: editOutgoingMessage,
    dismiss: dismissOutgoingMessage,
    endUndo: endSendUndo,
    renderUndo: renderSendUndo,
    putBack: putBackDismissedSend,
    forget: forgetDismissedSends,
    load: loadDismissedSends,
    dispose() {
      document.removeEventListener("keydown", onTrapTab);
      el("dismissed-send-close").removeEventListener("click", onClose);
      el("dismissed-send-copy").removeEventListener("click", onCopy);
      el("dismissed-send-restore").removeEventListener("click", onRestore);
    },
  };
}


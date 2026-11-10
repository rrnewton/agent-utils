// Placing what the stream left unplaced: the placing read of All, what its answer places, and
// the removals the page was told of (`#624 voice-page-modularity`, moved from the legacy script).
// Words, order and timing are as they were in the script. The arrival count and the placing
// read's flight record are core/sync owners made in the script and handed in.

import { answerUnplaced, createRemovals, heldAsSent, heldNow, newerPage, placeFromPageIn, timeOf, unplacedToAsk } from "../../core/channels/index.ts";
import type { ChannelCanon, CopyOrder, Removals } from "../../core/channels/index.ts";
import { DISCORD_PAGE_LIMIT } from "../../core/channels/index.ts";
import type { Message } from "../../contract/index.ts";

/** A placing read's answer, as this owner reads it. */
export interface PlacingPayload { delta?: { more?: boolean } | null; messages?: Message[]; [key: string]: unknown }

/** One placing read on its way, as this owner keeps it. */
export interface PlacingRead {
  key: string; channel: string; canon: ChannelCanon; sent: string | null; seen: number;
  sentAt: number; live: unknown; news: unknown; number: number;
  held: Map<string, Message>; removals: unknown;
}

/** What placing asks of the page. */
export interface PlacingDependencies {
  channelStore(): { current(): ChannelCanon };
  providerFacts: { threading(): boolean };
  unplacedArrivals(): { count(): number };
  placingReadInFlight(): { busy(): boolean; begin(channel: string): void; end(): void };
  readOrder(): { asked(): number };
  readHolds(): { channelWaits(channel: string): boolean };
  liveEvents(): { seen(channel: string): unknown; news(channel: string): unknown; removals(channel: string): unknown };
  abandonedReads(): { holdKey(): string; abandon(answer: unknown, read: unknown, land: (late: unknown) => void, settled: (landed: unknown, unused: unknown) => void): unknown };
  channelReads(): { busy(): boolean };
  copyLedger(): CopyOrder;
  timelineReader(): { readTimeline(path: string, opts: unknown): Promise<unknown> };
  columns(): { main: unknown; side: unknown };
  jumpNewestWanted(): { discord: number | boolean };
  timelineReconcileMs: number;
  readTimeoutMs(): number;
  hiddenNow(): boolean;
  eachColumn(act: () => void): void;
  el(id: string): HTMLElement;
  foldTimelineDelta(payload: unknown, sent: unknown, view: string, thread: unknown): boolean;
  foldTimelinePage(payload: unknown, replace: boolean, view: string, thread: unknown): void;
  hiddenEmpty(message: Message): boolean;
  hidingRead(): boolean;
  inColumn<T>(column: unknown, act: () => T): T;
  isCursorRefusal(error: unknown): boolean;
  noteArchived(payload: unknown, replace: boolean): void;
  projectView(): Message[] | null;
  readFilterKeeps(): (message: Message) => boolean;
  renderCachedTimeline(): void;
  renderChannelNavigation(): void;
  renderScrollTools(): void;
  saveChannelScope(): void;
  selectedChannelId(): unknown;
  selection(): { view: string; threadId: unknown };
  sentNumberOf(payload: unknown): number;
  settleAfterRead(shown: Message[], opts: unknown): void;
  syncUnreadReplies(): void;
  timelineRows(): { messages: Message[] };
  viewKey(view: string, threadId?: unknown): string;
  within<T>(ms: number, fn: (signal: AbortSignal) => Promise<T>, late: (answer: unknown) => unknown): Promise<T>;
  channelReadPosition(area: HTMLElement): Record<string, unknown>;
  catchUpFromAll(): void;
}

/** Placing what the stream left unplaced, and the removals record. */
export interface Placing {
  removalRecord: Removals;
  noteRemoved(canon: ChannelCanon, id: string, observedMs?: number | null): void;
  removedAfterRead(canon: ChannelCanon, id: string, versions: Parameters<Removals["removedAfterRead"]>[2]): boolean;
  believeCarried(canon: ChannelCanon, payload: PlacingPayload): void;
  removedUpstream(id: unknown): boolean;
  placeUnplacedWaits(channel: string): boolean;
  queuePlaceUnplaced(): void;
  placeUnplaced(): Promise<void>;
  landPlacingRead(payload: PlacingPayload, read: PlacingRead): boolean;
  placeFromAnswer(payload: PlacingPayload, read: PlacingRead): void;
  placeFromPage(payload: PlacingPayload, canon: ChannelCanon, number: number, held?: Map<string, Message> | null): boolean;
  followPlacement(): void;
}

/** Placing over `deps`. */
export function createPlacing(deps: PlacingDependencies): Placing {
  // --- placing what the stream left unplaced -------------------------------------------------------
  //
  // `#308 live-reply-in-main`. The owner, 2026-10-14: in a space where a bridge answers him in threads,
  // its threaded answer appeared live in MAIN, opening the thread did not show it, and a reload put it
  // inside the thread. The stream had carried the reply without its thread record, which drew it as a
  // Main row (`unplaced`), and the read behind the arrival was of Main, which never carries a thread
  // reply: a delta of Main left the row where it was, and a newest page of Main took it out of the store
  // without putting it anywhere. Only a read of the thread or of All could place it, and with the chat
  // service slow, none did until the reload read the thread in full.
  //
  // So once the channel's reads go quiet, the messages the stream left unplaced that those reads did not
  // place are placed by ONE read of All, which carries every message with its thread record: what
  // changed in All since its last read where it can be, else its newest page. Folded as a read of All
  // is, it moves each into its thread in every column (`followPlacement`), and brings the view on screen
  // up to date as any read of All does (`catchUpFromAll`). Not for a channel that is one thread, whose
  // Main holds every message in it, nor while the page is hidden, nor while another read of the channel
  // is on the wire: the read ending asks again. One at a time. A read that fails is asked again when the
  // next read of the channel ends, the poll's at the latest. A read of All that lands — this one, or the
  // reader's own — answers for every unplaced message that arrived before it was sent, carried or not:
  // one it does not carry has nothing there to place it by, and asking again would bring the same answer.
  //
  // AN ANSWER OF ALL THAT IS NOT FOLDED STILL PLACES WHAT IT CARRIES (`placeFromAnswer`). A newest page
  // of All is not folded over one sent after it, nor once the stream has delivered anything since it was
  // sent; a delta is not folded once All's cover has moved on from where it began, nor once the stream
  // has removed a message since it was sent, which it may carry and would put back (`noteLiveRemoval`);
  // neither once the store has gained, lost or replaced a message since by any other way (`heldAsSent`)
  // — the reader's own send, acknowledged, a message Main's poll brought, or an edit it brought — which
  // the answer would take out as deleted, put back, or undo with its older copy; a late answer is not
  // used once something newer has replaced it.
  // And behind a bridge that streams its answer by editing it, every placing read is out while an edit
  // arrives. Thrown away whole, each was asked again, and the answer stayed on Main until the channel
  // went quiet; marked as having answered without being folded, a reply that a page of All sent before
  // it arrived had taken out of the store was gone from Main, its thread and the store for minutes. So
  // each message left unplaced that such an answer carries takes the thread record it carries there, and
  // only that, as from an edit (`placeFromPage`): the answer may be older than the store's copy, which
  // keeps what it says. One the store no longer holds — taken out by a newest page of Main, which never
  // carries a thread reply, or by a newest page sent before it arrived — is put back as the answer
  // carries it, unless a newest page sent after the answer that speaks for it has landed, and so said it
  // is gone: one of All, or of Main for a message on the main channel. Which of two pages is the newer
  // is the order they were sent in, not landed in (`sentNumberOf`). Never one removed upstream: a
  // removal from the stream, or a delta's deletions, says the message is gone, and it is no longer left
  // unplaced (`forgetUnplaced`). A reply removed while the placing read was out used to come back in its
  // thread from that read's answer, counted as unread. And the read of All for Main's counts, which is
  // folded for what it brings, is folded only over the store's copies older than it, and never brings
  // back a message removed upstream since (`pageFold`, `#316 late-counts-resurrect`).
  //
  // NOR WHILE A READ OF THE CHANNEL THE PAGE GAVE UP ON MAY STILL BE ANSWERED (`remote-ops-audit` W7).
  // The server is still reading for it, and behind a bridge that read is a scan of the space: a read
  // beside it is the second scan W7 exists to prevent, and its late answer is used only if no read of the
  // channel was sent after it (`abandonedReadStands`), so a read beside it throws that answer away too.
  // Behind a slow bridge — newest pages taking longer than the page waits for them — that is the
  // ordinary case, and it is every message the stream left unplaced, a thread reply or one posted to the
  // channel itself. So the placing read waits while any view of the channel is held (`readHolds`): until
  // the read given up on answers or fails, when it is asked for at once (`abandonRead`), or the server's
  // bound has passed, or a live event says the service is answering. Given up on itself, it is kept the
  // same way: its late answer is used if nothing has replaced it, and still places what it carries if
  // something has, and until then the poll of the view on screen waits for it, since that answer brings
  // that view up to date as well. A late answer asks again for what it could not answer; a failure waits
  // for the next read of the channel to end.


  // Which unplaced messages no read of All has answered for (`unplacedToAsk`), a read of All answering for
  // them (`answerUnplaced`), and whether the store holds what it held when a read was sent (`heldAsSent`) are
  // web/src/core/channels's (`#624 voice-page-modularity`).

  // `forgetUnplaced` is web/src/core/channels's (`#624 voice-page-modularity`).

  /**
   * Message `id` is removed upstream (web/src/core/channels `Removals.noteRemoved`, `#315
   * removed-reply-target`), as of the read sent last (`readOrder`).
   *
   * @param {any} canon
   * @param {string} id
   * @param {number | null} [observedMs] when the server observed the removal
   */
  function noteRemoved(canon: ChannelCanon, id: string, observedMs: number | null = null) {
    removalRecord.noteRemoved(canon, id, deps.readOrder().asked(), observedMs);
  }

  /**
   * The removals the page was told of, and when the server observed each (web/src/core/channels
   * `createRemovals`, `#473 replication-c7-server-versions`). Here where the map it replaces stood.
   */
  const removalRecord = createRemovals();

  /** Whether a page with versions `versions` began before the server observed message `id`'s removal (`Removals.removedAfterRead`). */
  function removedAfterRead(canon: ChannelCanon, id: string, versions: Parameters<Removals["removedAfterRead"]>[2]) {
    return removalRecord.removedAfterRead(canon, id, versions);
  }

  /** A read sent after a removal that still carries the message says it is there after all (`Removals.believeCarried`). */
  function believeCarried(canon: ChannelCanon, payload: PlacingPayload) {
    removalRecord.believeCarried(canon, payload, () => deps.sentNumberOf(payload));
  }

  /** Whether message `id` of the selected channel is removed upstream, as far as this page knows (`noteRemoved`). */
  function removedUpstream(id: string) {
    return deps.channelStore().current().channel === String(deps.selectedChannelId()) && deps.channelStore().current().removed.has(String(id));
  }

  /**
   * Whether a read of `channel` the page gave up on may still be answered: a hold on any of its views
   * (`readHolds`), a placing read's own among them, that the server's bound has not ended and no live
   * news has lifted (`noteLiveEvent`). The placing read waits for it: see above.
   *
   * @param {string} channel
   */
  function placeUnplacedWaits(channel: string) {
    return deps.readHolds().channelWaits(channel);
  }

  /** `placeUnplaced`, once the read ending now has drawn what it brought. */
  function queuePlaceUnplaced() {
    if (!deps.providerFacts.threading() || deps.channelStore().current().unplaced.size === 0) return;
    Promise.resolve().then(placeUnplaced).catch(() => {});
  }

  /**
   * @typedef {import("./src/core/sync/index.ts").AbandonedRead & {canon: any, sent: string | null, seen: number, number: number, held: Map<string, any>,
   *   removals: number}} PlacingRead A placing read: the position in All it continues, or null for All's
   *   newest page, `unplacedArrivals` and `readOrder` when it was sent, the store's copies of
   *   the messages it held then (`heldAsSent`), and `deps.liveEvents().removals` for its channel then. Its `key` is
   *   the view on screen in the left column, whose poll waits for it once it is given up on.
   */

  /**
   * Read All to place what the stream left unplaced and no read has placed since: see above. What changed
   * in All since its last read, when All has a position this page read recently enough to continue from
   * (`#203 incremental-refresh`) — behind a bridge that is the cheap read, where All's newest page can be
   * a scan of the space — and otherwise, or when the server can no longer continue from it, the newest
   * page. A delta that says more follows is not followed: the newest page is read instead, as a refresh
   * does past its last delta page. Given up on, the read is kept, and its view held (`abandonRead`): its
   * late answer lands as one in time does (`landPlacingRead`).
   */
  async function placeUnplaced() {
    const channel = String(deps.selectedChannelId());
    if (!deps.providerFacts.threading() || !channel || deps.channelStore().current().channel !== channel || deps.channelStore().current().scope ||
        deps.placingReadInFlight().busy() || deps.channelReads().busy() || deps.hiddenNow() ||
        placeUnplacedWaits(channel)) return;
    if (unplacedToAsk(deps.channelStore().current()).length === 0) return;
    deps.placingReadInFlight().begin(channel);
    const canon = deps.channelStore().current();
    const all = canon.views.get(deps.viewKey("flat"));
    let sent = all && all.live && all.newest && Date.now() - (all.fullAt || 0) < deps.timelineReconcileMs
      ? all.newest : null;
    const base = `/api/v1/channels/${encodeURIComponent(channel)}/timeline?view=flat&limit=${DISCORD_PAGE_LIMIT}`;
    const key = deps.inColumn(deps.columns().main, deps.abandonedReads().holdKey);
    let again = false;
    try {
      for (;;) {
        /** @type {PlacingRead} */
        const read = { key, channel, canon, sent, seen: deps.unplacedArrivals().count(), sentAt: Date.now(),
          live: deps.liveEvents().seen(channel), news: deps.liveEvents().news(channel), number: 0,
          held: heldNow(canon), removals: deps.liveEvents().removals(channel) };
        const path = `${base}${sent ? `&after=${encodeURIComponent(sent)}` : ""}&reason=place`;
        /** @type {VibeTalk.TimelineResponse} */
        let payload: PlacingPayload;
        try {
          // A late answer that lands asks again for what it could not answer; a failure does not. One
          // that something newer has replaced still places what it carries (`placeFromAnswer`).
          const asked = deps.within(deps.readTimeoutMs(), (signal) => deps.timelineReader().readTimeline(path, { signal }),
            (answer) => deps.abandonedReads().abandon(answer, read, (late) => landPlacingRead(late as PlacingPayload, read), (landed, unused) => {
              if (unused) placeFromAnswer(unused as PlacingPayload, read);
              if (landed) queuePlaceUnplaced();
            }));
          read.number = deps.readOrder().asked();
          payload = (await asked) as PlacingPayload;
        } catch (error) {
          if (sent && deps.isCursorRefusal(error)) {
            sent = null;
            continue;
          }
          throw error;
        }
        if (deps.channelStore().current() !== canon || canon.channel !== channel || String(deps.selectedChannelId()) !== channel) return;
        if (sent && payload.delta && payload.delta.more) {
          sent = null;
          continue;
        }
        again = !landPlacingRead(payload, read);
        return;
      }
    } catch (_error) {
      // Asked again when the next read of the channel ends, and, once given up on, when the server has
      // stopped reading for it (`placeUnplacedWaits`).
    } finally {
      deps.placingReadInFlight().end();
      if (again) queuePlaceUnplaced();
    }
  }

  /**
   * The answer of the placing read `read` describes, in time or late (`abandonRead`), folded as a read of
   * All is: it places each message it carries in every column (`followPlacement`), and the view on screen
   * in each is as current as this read of All, as for any (`catchUpFromAll`, `#220 view-switch-instant`).
   *
   * NOT FOLDED: a delta that no longer continues All's cover — a read of All landed while it was out — or
   * a late one that says more follows, or one out as the stream removed a message, which it may carry
   * and would put back as a thread reply (`noteLiveRemoval`); a newest page when a newest page of All
   * SENT after it has landed (`newerPage`), which is the newer wherever it lands, while one sent before
   * it is the older and this page is folded over it; a newest page that lands after the stream has
   * delivered anything since it was sent, which as the newest page of All would take that message out
   * of the store as deleted upstream, and for which false is answered, to ask again for what arrived
   * since; and either, when the store no longer holds what it held when the read was sent
   * (`heldAsSent`). The store gains a message by other ways than the stream — the reader's own send,
   * acknowledged; a message the stream missed, brought by Main's poll; a read of a thread — and a newest
   * page, sent before, would take that one out as deleted too (`#310 stale-live-read-flicker`); one
   * taken out since, it would put back; and a copy replaced since — an edit Main's read brought while
   * the answer was out — either answer would undo with its older copy, until the next read to carry the
   * message: for a newest page of Main, never, once a message is a thread reply. Not folded, the answer
   * is still not thrown away (`placeFromAnswer`), and that never takes a message out of the store, nor
   * gives one anything but a thread record.
   *
   * @param {VibeTalk.TimelineResponse} payload
   * @param {PlacingRead} read
   * @returns {boolean}
   */
  function landPlacingRead(payload: PlacingPayload, read: PlacingRead) {
    const { canon, channel } = read;
    if (deps.channelStore().current() !== canon || canon.channel !== channel || String(deps.selectedChannelId()) !== channel) return true;
    const delta = Boolean(read.sent && payload.delta);
    const quiet = deps.liveEvents().seen(channel) === read.live;
    const unremoved = deps.liveEvents().removals(channel) === read.removals;
    const unchanged = heldAsSent(canon, read.held);
    let folded = false;
    if (delta) {
      folded = unremoved && unchanged && !payload.delta!.more && deps.foldTimelineDelta(payload, read.sent, "flat", null);
    } else if (quiet && unchanged && !newerPage(canon, "flat", read.number)) {
      deps.foldTimelinePage(payload, false, "flat", null);
      folded = true;
    }
    if (!folded) {
      placeFromAnswer(payload, read);
      return delta || quiet;
    }
    answerUnplaced(canon, read.seen);
    if (deps.inColumn(deps.columns().main, () => deps.selection().view === "flat")) {
      // On All: what the page says is read is what All's rows show, as for All's own read.
      deps.noteArchived(payload, false);
    } else {
      // Behind another view: All is as current as this read from its next showing.
      canon.views.get(deps.viewKey("flat"))!.landedHidden = true;
    }
    followPlacement();
    deps.eachColumn(deps.catchUpFromAll);
    return true;
  }

  /**
   * An answer of the placing read `read` describes that is not folded (`landPlacingRead`), or a late one
   * that something newer has replaced (`abandonedReadStands`): what it says of the messages the stream
   * left unplaced (`placeFromPage`), and it answers for the rest that arrived before it was sent as one
   * folded does — unless it is a delta that says more follows, which is not all that changed.
   *
   * @param {VibeTalk.TimelineResponse} payload
   * @param {PlacingRead} read
   */
  function placeFromAnswer(payload: PlacingPayload, read: PlacingRead) {
    const { canon, channel } = read;
    if (deps.channelStore().current() !== canon || canon.channel !== channel || String(deps.selectedChannelId()) !== channel) return;
    const placed = placeFromPage(payload, canon, read.number, read.held);
    if (!(read.sent && payload.delta && payload.delta.more)) answerUnplaced(canon, read.seen);
    if (placed) followPlacement();
  }

  /**
   * What `payload`, an answer of All that is not folded, says of the messages the stream left unplaced in
   * store `canon` (web/src/core/channels `placeFromPage`, which says how each is placed), with this page's
   * copy ledger. True when it placed any. `answeredThread`, `newerPage` and `newerPageSpeaksFor` are
   * web/src/core/channels's too.
   *
   * @param {any} payload
   * @param {any} canon
   * @param {number} number
   * @param {Map<string, any> | null} [held]
   * @returns {boolean}
   */
  function placeFromPage(payload: PlacingPayload, canon: ChannelCanon, number: number, held: Map<string, Message> | null = null) {
    return placeFromPageIn(payload, canon, number, held, deps.copyLedger());
  }

  /**
   * Draw each column showing the channel's messages from the store again where the store no longer
   * holds what it shows: a message moved into its thread (`#308 live-reply-in-main`), or a read of All
   * behind it corrected a row. As a refresh that keeps the reader's place draws (`#280 scroll-stability`):
   * the reader stays on the message they are reading, or on the newest line, and the jump to the newest
   * is offered only for a message newer than any the column showed — a message leaving Main for its
   * thread is not news there. Main's counts, the thread picker, the cards and an open Reply follow.
   */
  function followPlacement() {
    if (!deps.providerFacts.threading() || deps.channelStore().current().channel !== String(deps.selectedChannelId())) return;
    deps.eachColumn(() => {
      const projected = deps.selection().view === "threads" ? null : deps.projectView();
      if (!projected) return;
      const drawn = deps.timelineRows().messages;
      if (projected.length === drawn.length && projected.every((message, at) => message === drawn[at])) {
        deps.syncUnreadReplies();
        return;
      }
      const area = deps.el("scroll-area");
      const position = deps.channelReadPosition(area);
      const newestAt = timeOf(drawn[drawn.length - 1], "timestamp");
      deps.timelineRows().messages = projected;
      deps.renderCachedTimeline();
      const keeps = deps.readFilterKeeps();
      const shown = deps.timelineRows().messages.filter((message) => !deps.hiddenEmpty(message) && (!deps.hidingRead() || keeps(message)));
      const newestNow = timeOf(shown[shown.length - 1], "timestamp");
      const newer = newestNow > newestAt;
      deps.settleAfterRead(shown, { keepPosition: true, area, ...position, ownAct: !newer });
      // What left from under the newest line was counted on the jump as it arrived: it is not there now.
      const kept = new Set(projected.map((message) => String(message.id)));
      const gone = drawn.filter((message) => !kept.has(String(message.id)) &&
        !(timeOf(message, "timestamp") <= newestNow)).length;
      const counted = deps.jumpNewestWanted().discord;
      if (!newer && gone > 0 && typeof counted === "number") {
        deps.jumpNewestWanted().discord = counted > gone ? counted - gone : false;
        deps.renderScrollTools();
      }
    });
    deps.inColumn(deps.columns().main, deps.renderChannelNavigation);
    deps.saveChannelScope();
  }

  // --- sending what the outbox holds, and taking a send back ---------------------------------------

  return {
    removalRecord, noteRemoved, removedAfterRead, believeCarried, removedUpstream,
    placeUnplacedWaits, queuePlaceUnplaced, placeUnplaced, landPlacingRead, placeFromAnswer,
    placeFromPage, followPlacement,
  };
}

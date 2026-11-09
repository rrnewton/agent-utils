// A reply that is sent marks what it answers Done (`#256 reply-marks-read`, `#300
// reply-marks-read`, `#311 sent-read-waits-for-done`, `#245 done-feedback`, `#327
// done-races-remaining`, `#892 voice-page-optimistic-done`; `#624 voice-page-modularity`,
// moved from the legacy script into view/channel for the plan's step 17): the Done, by the
// Done button's own act, and then the chat service's own read marker moved through it where
// it can be. The columns, the store, the Done sets and the inbox writes are handed in; the
// words are the page's.

import { idsOf } from "./rows.ts";

/** The chat service's half of a mark (`markReadInService`). */
export interface ServiceMark {
  service: string;
  asked: boolean;
  marked: boolean;
  already: boolean;
  why: string;
}

/** A message heading a thread, as `rootThisStarts` reads the outbox. */
export interface SentReadOutboxEntry {
  channel: string;
  threadId?: string | null;
  continueIn?: string | null;
}

/** What the sent-read chain is handed by the page. */
export interface SentReadDependencies {
  el(id: string): HTMLElement;
  replyColumn(): unknown;
  mainColumn(): unknown;
  sideColumn(): unknown;
  sideShowing(): boolean;
  inColumn<T>(column: unknown, act: () => T): T;
  rowHolding(id: string): HTMLElement | null;
  threading(): boolean;
  channelCanon(): { channel: string; scope: unknown; dismissed: Set<string> };
  selectedChannelId(): unknown;
  readHiding(): boolean;
  doneArchived(): Set<string>;
  inboxLedger(): { add(kind: string, channel: string, ids: string[], owner: unknown): unknown; settle(kept: unknown): void };
  inboxWriteOwner(): unknown;
  queueInboxWrite(act: () => Promise<unknown>): Promise<unknown>;
  inboxPost(path: string, body: unknown): Promise<unknown>;
  toggleArchived(ids: readonly string[], row: HTMLElement | null): Promise<void>;
  bookmarkAnchor(channel: string): unknown;
  setStatus(text: string): void;
  doneSavingHas(id: string): boolean;
  doneSaved(ids: readonly string[], still: () => boolean): Promise<boolean>;
  providerCount(): number;
  providerOfChannel(channel: string): { upstream_read_mark_supported?: boolean } | null;
  upstreamReadMark(): boolean;
  heldUnansweredRoot(thread: string): { id: unknown } | null;
  outboxValues(): Iterable<SentReadOutboxEntry>;
  chatServiceOf(channel: string): string;
  api(path: string, options: unknown): Promise<{ outcome?: unknown } | null>;
  readableFailure(error: unknown, service: string): string;
  serviceReadWords(mark: ServiceMark): string;
}

/** The sent-read chain, as the page calls it. */
export interface SentRead {
  answeredRow(id: string): { column: unknown; row: HTMLElement } | null;
  doneWhereNotDrawn(id: string): boolean;
  answeredDone(id: string, found?: { column: unknown; row: HTMLElement } | null): boolean;
  doneHeldHere(ids: readonly string[]): boolean;
  markAnsweredDone(channel: string, id: string): Promise<boolean>;
  channelMarksUpstream(channel: string): boolean;
  rootThisStarts(channel: string, thread: string | null): string | null;
  channelIsOneThread(channel: string): boolean;
  serviceMarkNotAsked(outcome: unknown): string | null;
  markReadInService(channel: string, id: string): Promise<ServiceMark>;
  markSentRead(entry: { channel: string; marksRead: unknown }): Promise<void>;
}

/** Make the sent-read chain from the page's columns, store and Done sets. */
export function createSentRead(deps: SentReadDependencies): SentRead {
  // --- a reply that is sent marks what it answers Done ---------------------------------------------
  //
  // `#256 reply-marks-read`. The owner: "when we reply to a message the UI marks it as grey as though
  // it's read but the button in the upper right still implies it's unread! I don't like that. I think
  // we should be consistent. If reply marks as read we should actually change the state to read and
  // thus update the thread unread count."
  //
  // What the grey was: `data-replied` (`#84 reply-aware-dismissal`), the dimming of a message that some
  // loaded message answers, the outbox's acknowledged reply among them. It is an observation drawn on
  // the row and nothing more. It is not Done, so the row's Done button still offered Done, Hide read
  // kept the row, Fold read did not fold it, the root's count of unread replies on Main went on
  // counting it (`#222 unread-replies`), and the server was never told.
  //
  // So a reply that has been SENT marks the message it answers Done, by the Done button's own act
  // (`toggleArchived`). That gives the same server write, the same greying at once, the same Undo chip,
  // the same bookmark slide, and the same counts and read filters after it, in both columns. It marks
  // the message answered, as the row standing for it, and never the rest of its thread. Nothing is
  // marked when the send fails, and nothing when the message is Done already, so the Undo chip is not
  // taken over by a Done that changed nothing. Opening Reply marks nothing.
  //
  // `#300 reply-marks-read`. The owner, 2026-10-14: Reply on a message and opening its thread to write in
  // it both make a reply to the message before, and they marked read differently — Reply marked the
  // message, and the first reply written in a thread opened from the grey Thread marked nothing. The
  // rule agreed, decided when the send is queued and kept on the outbox entry as `marksRead`:
  //
  //   * THE FIRST REPLY THAT STARTS A THREAD marks its root, whichever way it was started: Reply on the
  //     root (its target is the root), or the thread opened and written in while it had no replies
  //     (`rootThisStarts`: the store holds the root and nothing else of the thread, its count says zero,
  //     and nothing written here is on its way there already).
  //   * A LATER MESSAGE in an existing thread's box marks nothing on its own.
  //   * A TARGETED REPLY — Reply, Quote reply, a row's Draft, in one column or docked in two — marks the
  //     message it answers, on Main or inside a thread. Only that message.
  //   * MARKING READ is what this page's own read action does for that message, and only once the send
  //     is confirmed (`markSentRead`): Done, as above; and, once that Done is saved — one still on its
  //     way when the send confirmed waited for (`#311 sent-read-waits-for-done`) — and where the
  //     channel's provider lets the server move its own read marker (the "⋯" menu's "In <service>
  //     only"), that marker moved through the message, asked `after_reply` so the server never moves
  //     it back behind a message already marked and never asks twice for one (src/reply_read.rs). Nothing
  //     is asked after a Done that failed. Never where that marker covers more than the
  //     channel — one thread of a larger space, or a service that cannot say — since nobody pressed
  //     anything to clear messages this page never received. Shift+E's read (`#295
  //     shift-e-read-collapse`, `readAndCollapse`) is this same mark, through `markReadInService`.
  //   * A VOICE AGENT'S REPLY follows the same rule, marked by the server, which posts it: see
  //     src/reply_read.rs. This page shows its Done at its next read of the channel.

  /**
   * The row standing for message `id` and the column drawing it: the column the last reply was opened
   * from first, then the left, then the right while it shows a thread. Null where no column draws it.
   *
   * @param {string} id
   * @returns {{column: any, row: HTMLElement} | null}
   */
  function answeredRow(id: string): { column: unknown; row: HTMLElement } | null {
    for (const column of [deps.replyColumn(), deps.mainColumn(), deps.sideColumn()]) {
      if (column === deps.sideColumn() && !deps.sideShowing()) continue;
      const row = deps.inColumn(column, () => deps.rowHolding(id) ||
        [...deps.el("pinned-log").children].find((one) => idsOf(one).includes(id)) || null);
      if (row) return { column, row: row as HTMLElement };
    }
    return null;
  }

  /**
   * Whether message `id`, drawn in no column, is Done by what the page holds: the channel's store, or
   * the left column's set. Under Hide read without threads a message not drawn has left the list, so
   * there is nothing to mark.
   *
   * @param {string} id
   */
  function doneWhereNotDrawn(id: string): boolean {
    if (deps.threading() && deps.channelCanon().channel === String(deps.selectedChannelId()) &&
        deps.channelCanon().dismissed.has(id)) return true;
    return deps.inColumn(deps.mainColumn(), () => deps.doneArchived().has(id) || (deps.readHiding() && !deps.threading()));
  }

  /**
   * Whether message `id`, which a send just confirmed, is Done here: every message of the row standing
   * for it, in the column drawing it (`answeredRow`, or `found` as that answered already), else, drawn in
   * no column, by what the page holds (`doneWhereNotDrawn`).
   *
   * @param {string} id
   * @param {{column: ColumnFields, row: HTMLElement} | null} [found]
   */
  function answeredDone(id: string, found: { column: unknown; row: HTMLElement } | null = answeredRow(id)): boolean {
    return deps.inColumn(found ? found.column : deps.mainColumn(), () => (found
      ? idsOf(found.row).every((one) => deps.doneArchived().has(one)) : doneWhereNotDrawn(id)));
  }

  /**
   * Whether every one of `ids`, of the channel on screen, is Done by what the page holds: the channel's
   * store where it keeps one, which a Done, a put back and a failed one taken back all move at once
   * (`markHeldDone`, `settleHeldDone`), else the left column's set — never by a row having left a list,
   * which a Done that failed does not always bring back. `#311 sent-read-waits-for-done`.
   *
   * @param {string[]} ids
   */
  function doneHeldHere(ids: readonly string[]): boolean {
    if (deps.threading() && deps.channelCanon().channel === String(deps.selectedChannelId())) {
      return ids.every((id) => deps.channelCanon().dismissed.has(String(id)));
    }
    return deps.inColumn(deps.mainColumn(), () => ids.every((id) => deps.doneArchived().has(String(id))));
  }

  /**
   * Mark Done message `id` of `channel`, which a send just confirmed marks read. See the section note.
   * Answers whether that Done is saved, which is when the chat service may be asked to mark it too
   * (`markSentRead`); a Done that fails throws.
   *
   * Done in the column that draws it (mirrored into the other by `mirrorArchived`), on every message
   * of its row, since the Done button reads a row half Done as unread and would go on offering Done.
   * Where no column draws it any more, the message alone. A reply that went to another channel than
   * the one on screen is marked on the server only: nothing of that channel is drawn, and an Undo here
   * would put back a message of this one. `#245 done-feedback`: the moment of the Done is drawn on the
   * row answered, the one found here, and on no row where no column draws it.
   *
   * `#311 sent-read-waits-for-done`. A MESSAGE DONE ALREADY is not marked again, but it may be drawn Done
   * by a Done still on its way to the server — `e`, the Done button, a swipe or Shift+E pressed just
   * before the send confirmed — since a message is drawn Done the moment it is marked. That Done is
   * waited for (`doneSaved`, as Shift+E waits for one, `#295 shift-e-read-collapse`) and the message
   * looked at again by what the page holds (`doneHeldHere`): false once that Done has failed, or been put
   * back meanwhile, and once the reader has gone to another channel, whose Done this page no longer
   * holds (a set kept over the change can still name it while its write is on its way). The chat
   * service's marker cannot be moved back, so nothing is asked of it for a Done this cannot say is saved.
   * Only a Done of its row actually on its way (`doneSaving`) is waited for and looked at again. Any other
   * is saved, and is asked of the service at once, as before: one saved before the send confirmed, and
   * one made before this page was loaded or on another device and read since — which, under Hide read
   * without threads, has taken the message off the list (`doneWhereNotDrawn`) and need not be in any set
   * `doneHeldHere` reads.
   *
   * @param {string} channel
   * @param {string} id
   * @returns {Promise<boolean>}
   */
  async function markAnsweredDone(channel: string, id: string): Promise<boolean> {
    if (channel !== String(deps.selectedChannelId())) {
      // Kept until answered, and sent again while unheard, as every Done is (`#892 voice-page-optimistic-done`).
      const kept = deps.inboxLedger().add("dismiss", channel, [String(id)], deps.inboxWriteOwner());
      try {
        await deps.queueInboxWrite(() => deps.inboxPost(`/api/v1/channels/${encodeURIComponent(channel)}/dismiss`, { messages: [id] }));
      } finally {
        deps.inboxLedger().settle(kept);
      }
      return true;
    }
    const found = answeredRow(id);
    const ids = found ? idsOf(found.row) : [id];
    if (answeredDone(id, found)) {
      // No Done of its own on its way: saved, or made before this page was loaded or elsewhere and read
      // since, which no set here need name (under Hide read without threads it has left the list).
      if (!ids.some((one) => deps.doneSavingHas(String(one)))) return true;
      return deps.doneSaved(ids, () => String(deps.selectedChannelId()) === channel && doneHeldHere(ids));
    }
    await deps.inColumn(found ? found.column : deps.mainColumn(), () => deps.toggleArchived(ids, found ? found.row : null));
    // `#240 reading-bookmark`. Send said where the way back is, and the Done's own words took the line.
    if (deps.bookmarkAnchor(channel)) deps.setStatus("b goes back to your bookmark.");
    // `#327 done-races-remaining`. Its own Done is saved, but Undo may have been pressed while it was:
    // a put back still on its way waited for, the message is looked at again as for a Done made before.
    return deps.doneSaved(ids, () => String(deps.selectedChannelId()) === channel && doneHeldHere(ids));
  }

  /**
   * Whether `channel`'s provider lets the server move its own read marker — what offers the "⋯" menu's
   * "In <service> only" — by the server's account. With several providers each channel answers for its
   * own, as the send service's posts-once does. `#300 reply-marks-read`.
   */
  function channelMarksUpstream(channel: string): boolean {
    if (deps.providerCount()) {
      const provider = deps.providerOfChannel(channel);
      return Boolean(provider && provider.upstream_read_mark_supported === true);
    }
    return deps.upstreamReadMark();
  }

  /**
   * The root a message written now in thread `thread` of `channel` would be the first reply to, by id,
   * or null: the store holds the thread's root and nothing else of it, its count says nobody has
   * answered it (`heldUnansweredRoot`, the rule the grey Thread is painted by), and nothing written here
   * is in the outbox for that thread already — the one before this, still on its way or sent and not
   * yet read back, is the thread's first reply, and this is a later one. `#300 reply-marks-read`.
   *
   * @param {string} channel
   * @param {string | null} thread
   */
  function rootThisStarts(channel: string, thread: string | null): string | null {
    if (!deps.threading() || !thread || deps.channelCanon().channel !== String(channel)) return null;
    const root = deps.heldUnansweredRoot(thread);
    if (!root) return null;
    const key = String(thread);
    for (const entry of deps.outboxValues()) {
      if (entry.channel === String(channel) && (entry.threadId === key || entry.continueIn === key)) return null;
    }
    return String(root.id);
  }

  /**
   * Whether `channel` is, by this page's last read of it, one thread of a larger space: a registration
   * narrowed to one conversation, which the timeline names as its `thread` (`channelCanon().scope`). Its
   * service's read marker covers the whole space. `#300 reply-marks-read`.
   *
   * @param {string} channel
   */
  function channelIsOneThread(channel: string): boolean {
    return deps.channelCanon().channel === String(channel) && Boolean(deps.channelCanon().scope);
  }

  /**
   * Why the chat service was not asked to move its read marker for a mark that follows a Done, by the
   * server's `outcome`, or by the page's own word for a channel it holds as one thread: the service's
   * marker covers more than the channel, or the service cannot say whether it does. Null for any other
   * outcome. `#300 reply-marks-read`.
   *
   * @param {unknown} outcome
   * @returns {string | null}
   */
  function serviceMarkNotAsked(outcome: unknown): string | null {
    if (outcome === "thread_channel") {
      return "this channel is one thread of a larger space there, and its read marker covers the whole space.";
    }
    if (outcome === "scope_unknown") {
      return "it cannot say whether its read marker covers more than this channel.";
    }
    return null;
  }

  /**
   * The chat service's half of marking message `id` of `channel` read, asked after its Done here, so
   * the two marks are one act: by Shift+E (`readAndCollapse`) and by a confirmed send (`markSentRead`).
   * `#300 reply-marks-read`, `#295 shift-e-read-collapse`. Both ask only once that Done is saved, a Done
   * still on its way waited for (`doneSaved`), and ask nothing after one that failed: Shift+E
   * (`readAndCollapse`), and a confirmed send since `#311 sent-read-waits-for-done` (`markAnsweredDone`).
   *
   * Where the channel's provider lets the server move its own read marker (what offers the "⋯" menu's
   * "In <service> only"), that marker moved through the message, asked `after_reply` so the server
   * never moves it back behind a message already marked and never asks the service twice for one
   * (src/reply_read.rs).
   *
   * Never where that marker is wider than the channel: on a channel that is one thread of a larger
   * space it would mark every earlier message of the whole space read in the service's own app,
   * messages this page never received, and nobody pressed anything to ask for that. The page does not
   * ask for a channel it holds as one thread (`channelIsOneThread`), and the server, which asks the
   * service what its marker covers, refuses the rest (`serviceMarkNotAsked`). The Done stands either way.
   *
   * Never a throw: the service's half failing is for the status line, not the error banner. Nobody
   * pressed anything for it alone, the Done it follows stands, and the chat service refusing it for a
   * while (Google Chat allows a bridge about two reads of its marker a minute) must not put a banner
   * over every mark.
   *
   * @param {string} channel
   * @param {string} id
   * @returns {Promise<{service: string, asked: boolean, marked: boolean, already: boolean, why: string}>}
   *   `asked`, whether this page asked the server at all; `marked`, whether the service's marker is
   *   through the message now; `already`, whether it was before this asked, by the server's answer that
   *   it had marked this message or a later one and asked the service nothing; `why`, the reason it is
   *   not marked, where there is one to say.
   */
  async function markReadInService(channel: string, id: string): Promise<ServiceMark> {
    const service = deps.chatServiceOf(channel);
    const none = { service, asked: false, marked: false, already: false, why: "" };
    if (!channelMarksUpstream(channel)) return none;
    if (channelIsOneThread(channel)) return { ...none, why: serviceMarkNotAsked("thread_channel") || "" };
    try {
      const answer = await deps.api(`/api/v1/channels/${encodeURIComponent(channel)}/upstream-read`, {
        method: "POST", body: { message_id: String(id), after_reply: true },
      });
      const outcome = answer && answer.outcome;
      const why = serviceMarkNotAsked(outcome);
      return { ...none, asked: true, marked: !why, already: outcome === "already_marked" || outcome === "behind",
        why: why || "" };
    } catch (error) {
      return { ...none, asked: true, why: deps.readableFailure(error, service) };
    }
  }

  /**
   * Mark read what `entry`, a send just confirmed, marks (`marksRead`): Done, and then the chat
   * service's own read marker moved through it where it can be (`markReadInService`). The second waits
   * for the first and is not asked when the Done failed, as "Mark read through here" goes. See the
   * section note. `#300 reply-marks-read`. `#311 sent-read-waits-for-done`: nor while a Done the message
   * is drawn with already is still on its way, nor once that one has failed (`markAnsweredDone`).
   *
   * @param {{channel: string, marksRead: string}} entry
   */
  async function markSentRead(entry: { channel: string; marksRead: unknown }): Promise<void> {
    const id = String(entry.marksRead);
    if (!(await markAnsweredDone(entry.channel, id))) return;
    const mark = await markReadInService(entry.channel, id);
    // Nothing asked: the Done's own words already say it was not marked in the chat service.
    if (!mark.asked) return;
    // Said over the Done's own words, which say it was not marked in the chat service. Only for the
    // channel on screen: a reply that finished after the reader moved on says nothing over another.
    if (entry.channel !== String(deps.selectedChannelId())) return;
    deps.setStatus(deps.serviceReadWords(mark) + (deps.bookmarkAnchor(entry.channel) ? " b goes back to your bookmark." : ""));
  }


  return {
    answeredRow, doneWhereNotDrawn, answeredDone, doneHeldHere, markAnsweredDone,
    channelMarksUpstream, rootThisStarts, channelIsOneThread, serviceMarkNotAsked,
    markReadInService, markSentRead,
  };
}

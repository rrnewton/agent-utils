// Summaries, asked for as you scroll (`#49 cached-summaries`, `#564 message-summary-toggle`; `#624
// voice-page-modularity`, moved from the legacy script into core/channels). The one owner of what the
// server has said about each row's summary, which rows have been asked about, the requests waiting and
// on the wire, the failures not yet reported, and whether this server can summarise at all. Which rows
// are on screen, drawing a row's summary line, and the chip and the standing note are the page's; the
// words are web/src/present/rows's. Why the mode is the owner's setting, one request per row, only what
// is looked at, and a failure shown on its row, is told where the legacy script opens the section.

import type { ClockPort, TimerHandle } from "../../ports/index.ts";

/**
 * How many summary requests may be on the wire at once. `#564 message-summary-toggle`.
 *
 * Every row on screen used to be asked about at once, a dozen or more, and the server paces its
 * model calls (15 a minute after a burst of 3 by default, `summaries.llm`) and turns away AT ONCE
 * a caller whose turn would come after its patience runs out. On the live instance that was 27 of
 * 95 summary requests answered 503 with no model call made, every one drawn as "summary failed".
 * Two at a time never queue long enough there to be turned away, and the rows still fill in order.
 */
const SUMMARY_IN_FLIGHT = 2;

/**
 * What the server has said about one row.
 *
 * `{ text }` with a string is a summary. `{ text: null }` is a settled "there is no summary for
 * this" — the server's own threshold, which is allowed to be stricter than the page's. `failed`
 * marks the third case, which is NOT settled: see `setMode`. A failed entry also carries `why`
 * (the server's own sentence, shown on the row's `title`) and `code` (the machine-readable one,
 * which is what tells a broken DEPLOYMENT apart from a broken attempt).
 */
export interface HeldSummary {
  readonly text: string | null;
  readonly failed?: boolean;
  readonly why?: string;
  readonly code?: string;
}

/** A row to ask about: its first message and the rest it stands for. */
export interface SummaryTarget {
  readonly id: string;
  readonly also?: readonly string[] | null;
}

/** The server's answer, as far as these read it. */
export interface SummaryAnswer {
  readonly backend?: unknown;
  readonly generated_in_ms?: unknown;
  readonly state?: unknown;
  readonly summary?: unknown;
}

/** A refused request, as the page's API client throws it. */
interface SummaryRefusal {
  readonly code?: string;
  readonly message?: string;
  readonly detail?: string;
}

/**
 * The key a row's summary is held under: every id the row stands for, in order. Not its first id
 * alone, because the same first message heads different rows in different views — a thread's root
 * is a row of its own on Main and the head of its long post's parts in the thread (`#463
 * slack-text-fidelity`) — and each row's summary is of the text that row draws.
 */
const summaryKey = (entry: SummaryTarget): string => [entry.id, ...(entry.also || [])].join(" ");

/** What the summaries ask of the page. */
export interface SummariesDependencies {
  readonly clock: ClockPort;
  /** The channel the current column shows (`selectedChannelId`), or "" with none. */
  channel(): string;
  /** The thread message `id` is in, in the current column (`threadForMessageId`), or null. */
  threadOf(id: string): string | null;
  /** Ask the server for one row's summary; rejects with the server's refusal. */
  fetch(channel: string, id: string, also: readonly string[], thread: string | null): Promise<SummaryAnswer | null>;
  /** Bring every row's summary line into agreement with what is held (`renderSummaries`). */
  render(): void;
  /** Say that `count` rows could not be summarised, the latest because `why`. */
  failuresReported(count: number, why: string): void;
  /** Ask about the rows on screen again, now (`requestVisibleSummaries`). */
  askAgain(): void;
}

/** The summaries' commands and questions. */
export interface Summaries {
  /** Is the reader in summary mode? The owner's synced setting, unless the server cannot summarise. */
  mode(): boolean;
  /** What the server has said about the row under `key`, or undefined before it has. */
  held(key: string): HeldSummary | undefined;
  /** What the server says produced the summaries, quoted from its own answer, or "". */
  backend(): string;
  /** Why this SERVER cannot summarise anything at all, or "" while it can. */
  unavailable(): string;
  /** Why the server says, in client-config, it cannot summarise at all, or "" when it may. */
  disabledByServer(): string;
  /** Client-config said so. */
  setDisabledByServer(why: string): void;
  /** How long the generated summaries took, in the order they were measured. */
  times(): readonly number[];
  /**
   * Enter or leave the mode. False where the server has disabled it and nothing changed; otherwise
   * whether this is a re-check, which the page asks with ONE request rather than one per row.
   */
  setMode(on: boolean): { readonly entered: boolean; readonly recheck: boolean };
  /** Ask about `targets` not yet asked about, at most `limit` of them. */
  ask(targets: Iterable<SummaryTarget>, limit?: number): void;
  /**
   * The server's settings were read or written, and `heldOn` says whether it holds summaries on: true
   * where rows it refused while it held them off were forgotten, and the page asks again.
   */
  settingLanded(heldOn: () => boolean): boolean;
}

/** Off, nothing held and nothing asked. */
export function createSummaries(deps: SummariesDependencies): Summaries {
  let summaryMode = false;

  /**
   * Rows the server refused because the owner's setting, as IT holds it, is off: this page turned it
   * on and the write has not landed yet, or the two have not converged, or this token may only read
   * and cannot write it. Not a failure and not shown: the rows keep their opening lines. They STAY
   * asked — a refusal un-asked would be asked again on every scroll and every redraw, forever, on a
   * page whose "on" can never reach the server — and are asked again only once the server is seen to
   * hold the setting on (`settingLanded`). `#564 message-summary-toggle`.
   */
  const summariesRefusedOff = new Set<string>();

  /** What the server has said about each row, by `summaryKey`. */
  const summaries = new Map<string, HeldSummary>();

  /** Every row a request has gone out for, by `summaryKey`. Consulted before asking, so one row is one ask. */
  const summariesAsked = new Set<string>();

  /**
   * What the server says produced the summaries, quoted from its own answer.
   *
   * Empty until something has answered, and the note says less while it is. Every summary is a
   * MODEL'S reading of somebody else's message, produced by a vendor this deployment pays, so a page
   * that showed those short lines without naming their author would be presenting a paraphrase as if
   * it were the message. The name is quoted from the server rather than written here, so it cannot
   * go on describing a summariser this deployment stopped running.
   */
  let summaryBackend = "";

  /**
   * Why this SERVER cannot summarise anything at all, or "" while it can.
   *
   * Set from `summarizer_not_configured`, which is not a fact about one message: it says the
   * deployment has no ElevenLabs credentials, so every request after it would buy the same refusal.
   * Two things follow, and they are deliberately different from the per-row failure above:
   *
   *   * NOTHING MORE IS ASKED. The page's `summaryTargets` returns nothing while this is set, so
   *     scrolling through a thousand-message channel costs the one request that found out, not one
   *     per row.
   *   * IT IS SAID IN THE STANDING NOTE, not only in the transient pill. A permanent fact about the
   *     deployment has to be readable after the six seconds are up.
   *
   * The rows that WERE asked still say so. "A summary was attempted here and did not arrive" is true
   * of them whatever the reason, and the deployment-level explanation is what the note is for.
   */
  let summariesUnavailable = "";

  /** True while ONE request is deciding whether an unavailable server can summarise again. */
  let summaryRecheck = false;

  /**
   * Why the SERVER says it cannot summarise at all, from client-config, or "" when it may.
   *
   * Known before anything is asked, so the control is disabled up front with this as its reason:
   * switching summaries on and watching every long row fail after several seconds is the
   * experience this replaces.
   */
  let summariesDisabledByServer = "";

  /**
   * How long the summaries actually took, in the order they were measured.
   *
   * The owner asked for this as an EXPERIMENT: a round trip to the ElevenLabs agent is expected to
   * beat a full-size model with a harness, and the way to find out is to measure it rather than to
   * reason about it. The server times each generation and reports `generated_in_ms`; without this
   * the number existed and nobody could see it.
   *
   * Only GENERATED summaries are recorded. A cache hit and a below-threshold answer both report no
   * time at all, and averaging a zero into them would report a backend as faster than it is —
   * precisely the wrong direction for a measurement meant to inform a choice.
   */
  const summaryTimes: number[] = [];

  /** Summary requests on the wire now. */
  let summariesInFlight = 0;

  /** Rows asked about and not yet sent, oldest first. */
  const summaryQueue: { id: string; also: readonly string[]; key: string; channel: string; thread: string | null }[] = [];

  /**
   * The rows that have failed since the last report, and the one timer that will report them.
   *
   * Twenty rows on screen is twenty requests, and a server that is down fails all twenty. Reported
   * one at a time that is twenty status writes racing each other through a six-second pill, of which
   * the reader sees the last — and the last one says "one message", which is the wrong number by
   * nineteen. Held here and flushed once, at the end of the turn, so the sentence can count.
   */
  const pendingSummaryFailures = new Set<string>();
  let summaryFailureTimer: TimerHandle | null = null;

  /**
   * Queue one row. Its channel and thread are read NOW, in the column that asked: a queued request is
   * sent later, from whichever column is current then, and on a two-column desk the other column's
   * thread would be the wrong one (`threadForMessageId` reads the current column).
   */
  function queueSummary(id: string, also: readonly string[], key: string): void {
    summaryQueue.push({ id, also, key, channel: deps.channel(), thread: deps.threadOf(id) });
    pumpSummaries();
  }

  /** Send queued requests while there is room. One the mode or the channel has left behind is forgotten. */
  function pumpSummaries(): void {
    while (summariesInFlight < SUMMARY_IN_FLIGHT && summaryQueue.length > 0) {
      const next = summaryQueue.shift() as (typeof summaryQueue)[number];
      if (!summaryMode || next.channel !== deps.channel()) {
        summariesAsked.delete(next.key);
        continue;
      }
      summariesInFlight += 1;
      fetchSummary(next.id, next.also, next.key, next.channel, next.thread).finally(() => {
        summariesInFlight -= 1;
        pumpSummaries();
      });
    }
  }

  /**
   * The server answered something other than "I have no summariser".
   *
   * Only interesting while a re-check is in flight: it means the deployment can summarise again, so
   * the rest of what the reader is looking at — which the single-request re-check deliberately did
   * not ask about — is asked about now rather than waiting for them to scroll.
   *
   * A re-check that FAILS deliberately does not clear the flag here, and does not need to: the
   * reader's next press recomputes it, and until then the flag can only be spent by an answer that
   * says this server is working, which is exactly the condition it is waiting for.
   */
  function summaryRecheckAnswered(): void {
    if (!summaryRecheck) {
      return;
    }
    summaryRecheck = false;
    deps.askAgain();
  }

  async function fetchSummary(id: string, alsoIds: readonly string[], key: string, channel: string,
    thread: string | null): Promise<void> {
    if (!channel) {
      // Never asked, so not left asked: the row is asked about when a channel is there.
      summariesAsked.delete(key);
      return;
    }
    let payload: SummaryAnswer | null = null;
    try {
      payload = await deps.fetch(channel, id, alsoIds, thread);
    } catch (thrown) {
      const error = thrown as SummaryRefusal;
      // `#564 message-summary-toggle`. The server holds the owner's setting off: not a failure, and
      // asked again once this page's write of it lands.
      if (error.code === "summaries_off") {
        summariesRefusedOff.add(key);
        return;
      }
      // The CODE, not the sentence. `summarizer_not_configured` is a fact about the deployment and
      // `summarizer_error` is a fact about this attempt; the two are told apart on the machine
      // string the server sends for exactly that purpose, never on the prose beside it.
      summaryFailed(key, error.message, error.code, error.detail);
      return;
    }
    summaryRecheckAnswered();
    if (payload && payload.backend) {
      summaryBackend = payload.backend as string;
    }
    // Absent on a cache hit and on a below-threshold answer, and absent is not zero: neither of
    // those asked the vendor anything, so neither is evidence about how fast the vendor is.
    if (payload && typeof payload.generated_in_ms === "number") {
      summaryTimes.push(payload.generated_in_ms);
    }
    // `below_threshold` is an ANSWER, not an omission: the server considers the message short
    // enough to read as it is, and a shortened copy of something already short would be a claim
    // that work was done. It is SETTLED — the row keeps its own opening lines and is never asked
    // about again — which is the whole reason the STATE decides rather than the presence of text.
    if (payload && payload.state === "below_threshold") {
      summaries.set(key, { text: null });
      deps.render();
      return;
    }
    // Anything else without usable text is a server this page cannot understand. That is a
    // FAILURE, not a verdict that the message is short: it is worth retrying, and reading it as
    // "below threshold" would quietly file every malformed answer as a decision nobody made.
    const said = payload ? payload.summary : null;
    if (typeof said !== "string" || !said) {
      summaryFailed(key, "the server answered without a summary");
      return;
    }
    summaries.set(key, { text: said });
    deps.render();
  }

  /**
   * One message could not be summarised.
   *
   * Deliberately NOT `guardQuietly`, and deliberately not the sticky `#error` panel: taking the
   * channel away because one row out of fifty could not be condensed is a worse answer than the row
   * the reader would have had with the mode off, and that row is exactly what they keep. The body is
   * never replaced.
   *
   * What CHANGED, at the owner's asking: the failure is no longer only a sentence that disappears.
   * The row itself says "summary failed" until it is answered: small and muted since `#564
   * message-summary-toggle`, never a surface that hides its read state. The earlier reading —
   * that a failure should be quiet because the fallback is acceptable — confused "the reader is not
   * blocked" with "the reader need not be told", and the standing complaint about this view is that
   * nothing in it is noticeable enough. A per-row mark plus one counted pill is loud where the
   * failure is and quiet everywhere else; the full-width panel would still be the wrong answer.
   *
   * `failed` also marks the entry as retryable — see `setMode`.
   */
  function summaryFailed(key: string, why?: string, code?: string, detail?: string): void {
    summaries.set(key, { text: null, failed: true, why: why as string, code: code as string });
    // A DEPLOYMENT-LEVEL refusal, not a row-level one: this server has no summariser configured, so
    // every other row would buy the same answer. The rows already asked about still say so — a
    // summary really was attempted for them and really did not arrive — but nothing further is
    // asked, and the reason is stated where it will still be readable in a minute.
    //
    // The server's PROSE, not the whole `HTTP 503 summarizer_not_configured: …` line: this one goes
    // into a standing sentence a reader has to parse, rather than onto a status line where the code
    // is the useful half.
    if (code === "summarizer_not_configured") {
      summariesUnavailable = detail || why || "this server has no summariser configured.";
    }
    pendingSummaryFailures.add(key);
    if (summaryFailureTimer === null) {
      summaryFailureTimer = deps.clock.setTimeout(reportSummaryFailures, 0);
    }
  }

  /** Draw every row that failed, and say it ONCE. */
  function reportSummaryFailures(): void {
    summaryFailureTimer = null;
    // Only the ones still failed. Re-entering the mode between the failure and this flush forgets
    // them on purpose (see `setMode`), and reporting a row that is being retried would be
    // reporting the past.
    const failed = [...pendingSummaryFailures].filter((id) => {
      const held = summaries.get(id);
      return held !== undefined && held.failed === true;
    });
    pendingSummaryFailures.clear();
    if (failed.length === 0) {
      return;
    }
    // ONE redraw for the whole batch, and it is what puts the words on the rows: see the page's
    // `applySummaryState`. Inside `preservingScroll`, because a failed row grows by the height of
    // its mark and rows above the viewport are exactly the case the browser cannot anchor.
    deps.render();
    if (summariesUnavailable) {
      // Said in the standing note the redraw has just written, and deliberately not ALSO in the
      // pill. A permanent fact about the deployment stated in a message that erases itself is a
      // fact the reader is invited to miss, and stating it twice trains them to dismiss both.
      return;
    }
    // The most recent reason, and the rows carry their own on `title`. A burst almost always shares
    // one cause; naming that one beats concatenating twenty copies of it into the status line.
    const why = (summaries.get(failed[failed.length - 1] as string) as HeldSummary).why;
    deps.failuresReported(failed.length, why as string);
  }

  return {
    mode: () => summaryMode,
    held: (key) => summaries.get(key),
    backend: () => summaryBackend,
    unavailable: () => summariesUnavailable,
    disabledByServer: () => summariesDisabledByServer,
    setDisabledByServer(why) {
      summariesDisabledByServer = why;
    },
    times: () => summaryTimes,
    setMode(on) {
      if (on && summariesDisabledByServer) return { entered: false, recheck: false };
      summaryMode = on;
      if (!on) {
        // Nothing still waiting to be sent is sent: off costs nothing from the moment it is off. Rows
        // the server refused are forgotten too, so turning it on again asks about them.
        for (const queued of summaryQueue.splice(0)) summariesAsked.delete(queued.key);
        for (const key of summariesRefusedOff) summariesAsked.delete(key);
        summariesRefusedOff.clear();
      }
      // A DEPLOYMENT-LEVEL refusal is re-checked with ONE request, never with one per visible row.
      // The credentials may have been fixed since — that is the whole reason to re-check at all — but
      // the overwhelmingly likely answer is the same refusal, and twenty of those cost this server
      // twenty Discord window fetches to arrive at. If the one comes back healthy,
      // `summaryRecheckAnswered` asks about the rest immediately, so the reader is not made to scroll.
      const recheck = on && summariesUnavailable !== "";
      summaryRecheck = recheck;
      if (on) {
        summariesUnavailable = "";
        // A failure is not a verdict. Re-entering the mode is the reader asking again, and without
        // this one flaky response would leave that row plain until the page is reloaded — while a
        // below-threshold answer, which is settled, stays settled and is never re-asked.
        for (const [id, held] of [...summaries]) {
          if (held.failed) {
            summaries.delete(id);
            summariesAsked.delete(id);
          }
        }
      }
      return { entered: true, recheck };
    },
    ask(targets, limit = Infinity) {
      let issued = 0;
      for (const entry of targets) {
        if (issued >= limit) {
          return;
        }
        const key = summaryKey(entry);
        if (summariesAsked.has(key)) {
          continue;
        }
        // Marked BEFORE the await, not in the handler: the scroll listener fires again long before a
        // response lands, and a record written on completion would let one row issue a request per
        // scroll event — the exact thing the issue names.
        summariesAsked.add(key);
        issued += 1;
        queueSummary(entry.id, entry.also || [], key);
      }
    },
    settingLanded(heldOn) {
      if (summariesRefusedOff.size === 0) return false;
      if (!heldOn() || !summaryMode) return false;
      for (const key of summariesRefusedOff) summariesAsked.delete(key);
      summariesRefusedOff.clear();
      return true;
    },
  };
}

// What the page and the tests use of it.
export { SUMMARY_IN_FLIGHT, summaryKey };

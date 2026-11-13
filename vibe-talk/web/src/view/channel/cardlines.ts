// Thread card lines and how much a quote must cover (`#624 voice-page-modularity`, moved from the
// legacy script into view/channel): where a card stops in its root, its head and continuation,
// whether a summary is on its way, what a card draws, and what a reply marks read. Why each
// behaves as it does is told where the legacy script held them. K and the quote share stay their
// settings owners', instantiated by the page and handed in.

import type { Message } from "../../contract/index.ts";
import { cardProxyLines, foldedCardText, proxyLinesOf } from "../../present/rows/index.ts";
import { cardStubText, cardUnits, keepCardUnits } from "./cardtext.ts";
import { quotePosition, quoteShare, replyShouldMarkRead } from "../../present/rows/index.ts";
import type { MessageBodies } from "./body.ts";
import { createSummaryWatch } from "./summarywatch.ts";
import type { CardChoice } from "./packcontrols.ts";

/** K in force, as far as a card reads it (core/settings owns it). */
export interface CardMaxLinesLike {
  current(): number;
  apply(value: unknown): number;
}

/** The quote share in force, as far as a reply reads it (core/settings owns it). */
export interface QuoteReadPercentLike {
  current(): number;
}

/** The Mark read on reply switches, as far as a reply reads them (core/settings owns them). */
export interface ReplyMarkSwitchesLike {
  on(): boolean;
}

/** The read mode in force, as far as a reply reads it: `readModes.mode()` (`show` or `fold`). */
export interface ReadModeLike {
  mode(): string;
}


/** A card's cut: after its first `head` units, or the whole root clamped, or a stub. */
export interface CardCut {
  head: number;
  whole: boolean;
  stub?: boolean;
}

/** What the card lines ask of the page. */
export interface CardLinesDependencies {
  readonly cardMaxLines: CardMaxLinesLike;
  readonly quoteReadPercent: QuoteReadPercentLike;
  readonly markReadOnReply: ReplyMarkSwitchesLike;
  readonly markReadQuoteGuard: ReplyMarkSwitchesLike;
  readonly markReadFoldGuard: ReplyMarkSwitchesLike;
  readonly markReadNonquote: ReplyMarkSwitchesLike;
  readonly markReadFinalGuard: ReplyMarkSwitchesLike;
  readonly markReadFirstGuard: ReplyMarkSwitchesLike;
  readonly markReadFirstPercent: QuoteReadPercentLike;
  readonly markReadLongWords: QuoteReadPercentLike;
  readonly readMode: ReadModeLike;
  bodies(): MessageBodies;
  rawTextOf(message: Message): string;
  headingOf(message: Message): { cut: number } | null;
  threadNames(): boolean;
  cardIsSummarized(choice: CardChoice): boolean;
  pendingInto(thread: unknown): number;
  heldMessages(): Message[];
  threadOf(message: unknown): string | null;
  threadWords(choice: CardChoice): { named: string; said: string };
  threadName(choice: CardChoice, chars: number): string;
  setTimer(run: () => void, ms: number): unknown;
  clearTimer(timer: unknown): void;
  guardQuietly<T>(fn: () => T): (...args: unknown[]) => Promise<unknown>;
  view(): string;
  selectedChannelId(): string;
  forgetCards(): void;
  clearThreadCards(): void;
  redrawCards(): void;
  keepingCardPlace(fn: () => void): void;
  savedLines(n: number): void;
  summaryWatchFired(): Promise<void>;
  stepLines(direction: 1 | -1): void;
  quotePercentChanged(value: unknown): unknown;
  stepPercent(direction: 1 | -1): void;
  renderedText(body: HTMLElement): string;
}

/** The card lines' commands and questions. */
export interface CardLines {
  cardMaxLinesChanged(value: unknown): number;
  stepCardMaxLines(direction: 1 | -1): void;
  quoteReadPercentChanged(value: unknown): unknown;
  stepQuoteReadPercent(direction: 1 | -1): void;
  replyMarksRead(target: { id: unknown; content: string }, text: string): string | null;
  rootOverflowsCard(root: Message | null | undefined): boolean;
  cardCut(root: Message): CardCut | null;
  cardHead(root: Message, cut: CardCut | null): HTMLElement;
  continuationOf(root: Message, cut: CardCut): Message;
  cardPending(choice: CardChoice | null): boolean;
  cardView(choice: CardChoice): { mode: string; text: string; cut: CardCut | null; body: () => HTMLElement | null };
  cutForCard(lines: number[], marks?: number[], wholes?: boolean[], headings?: boolean[]): CardCut | null;
  rootInward(choice: CardChoice): boolean;
  cardSplit(choice: CardChoice | null): CardCut | null;
  watchPendingSummaries(choices: CardChoice[], source: string): void;
}

const SUMMARY_WAIT_MS = 60000;
const SUMMARY_READ_STEPS_MS: readonly number[] = [2000, 5000, 10000, 20000, 40000];

/** Cards at K lines, nothing waiting. */
export function createCardLines(deps: CardLinesDependencies): CardLines {
  const cardMaxLines = deps.cardMaxLines;

  function cardMaxLinesChanged(value: unknown): number {
    let n = cardMaxLines.current();
    deps.keepingCardPlace(() => { n = cardMaxLines.apply(value); });
    deps.savedLines(n);
    return n;
  }

  function stepCardMaxLines(direction: 1 | -1): void {
    const prefs = deps.stepLines(direction);
    void prefs;
  }

  function quoteReadPercentChanged(value: unknown): unknown {
    return deps.quotePercentChanged(value);
  }

  function stepQuoteReadPercent(direction: 1 | -1): void {
    deps.stepPercent(direction);
  }

  function replyMarksRead(target: { id: unknown; content: string }, text: string): string | null {
    const whole = deps.rawTextOf(target as Message);
    const choices = {
      enabled: deps.markReadOnReply.on(),
      quoteGuard: deps.markReadQuoteGuard.on(),
      foldGuard: deps.markReadFoldGuard.on(),
      inFold: deps.readMode.mode() === "fold",
      markNonQuote: deps.markReadNonquote.on(),
      finalGuard: deps.markReadFinalGuard.on(),
      firstGuard: deps.markReadFirstGuard.on(),
      firstPercent: deps.markReadFirstPercent.current(),
      longWords: deps.markReadLongWords.current(),
    };
    return replyShouldMarkRead(choices, quoteShare(text, whole) as never, deps.quoteReadPercent.current(), quotePosition(text, whole) as never) ? String(target.id) : null;
  }

  function rootOverflowsCard(root: Message | null | undefined): boolean {
    return Boolean(root) && (cardProxyLines(proxyLinesOf(deps.rawTextOf(root as Message)) as never) as number) > cardMaxLines.current();
  }

  const cardCuts = new WeakMap<object, { lines: number; cut: CardCut | null; text: string }>();

  function cardCut(root: Message): CardCut | null {
    return cardCutEntry(root).cut;
  }

  function cardCutEntry(root: Message): { lines: number; cut: CardCut | null; text: string } {
    const held = cardCuts.get(root);
    if (held && held.lines === cardMaxLines.current()) return held;
    const units = cardUnits(deps.bodies().messageBody(root) as never) as Array<{ lines: number; mark: number; whole: boolean; heading: boolean; nodes: Node[] }>;
    const cut = cutForCard(units.map((unit) => unit.lines), units.map((unit) => unit.mark), units.map((unit) => unit.whole),
      units.map((unit) => unit.heading));
    const entry = { lines: cardMaxLines.current(), cut, text: foldedCardText(renderedTextOf(cardHead(root, cut))) as string };
    cardCuts.set(root, entry);
    return entry;
  }

  function renderedTextOf(box: HTMLElement): string {
    return deps.renderedText(box);
  }

  function cutForCard(lines: number[], marks: number[] = [], wholes: boolean[] = [], headings: boolean[] = []): CardCut | null {
    const total = lines.reduce((sum, n) => sum + n, 0);
    if (total <= cardMaxLines.current()) return null;
    let head = 0;
    let used = 0;
    while (head < lines.length && used + (lines[head] as number) <= cardMaxLines.current()) {
      used += lines[head] as number;
      head += 1;
    }
    while (head > 0 && used + (marks[head - 1] || 0) > cardMaxLines.current()) {
      head -= 1;
      used -= lines[head] as number;
    }
    while (head > 1 && headings[head - 1]) {
      head -= 1;
      used -= lines[head] as number;
    }
    if (head === 0) return wholes[0] ? { head: 0, whole: false, stub: true } : { head: 1, whole: true };
    if (total - used < 2) return null;
    return { head, whole: false };
  }

  function cardHead(root: Message, cut: CardCut | null): HTMLElement {
    const box = deps.bodies().messageBody(root);
    if (cut && cut.stub) {
      const block = (cardUnits(box as never) as Array<{ nodes: Node[] }>)[0]?.nodes[0] as Node;
      const line = document.createElement("p");
      line.className = "card-stub";
      line.append(document.createTextNode(`${cardStubText(block as never)} `), deps.bodies().cardEllipsis());
      box.replaceChildren(line);
      return box;
    }
    if (cut) {
      keepCardUnits(box as never, ((at: number) => at < cut.head) as never);
      if (!cut.whole) deps.bodies().appendCardEllipsis(box);
    }
    deps.bodies().unlinkCardBody(box);
    return box;
  }

  function continuationOf(root: Message, cut: CardCut): Message {
    const heading = deps.headingOf(root);
    return { ...root, heading: null, cardRestFrom: cut.head, cardRestCut: heading ? heading.cut : 0 } as Message;
  }

  function cardPending(choice: CardChoice | null): boolean {
    if (!deps.threadNames() || !choice || !choice.root || deps.cardIsSummarized(choice)) return false;
    if (!Number.isFinite(choice.at) || Date.now() - (choice.at as number) >= SUMMARY_WAIT_MS) return false;
    if (!cardCut(choice.root) || !rootOverflowsCard(choice.root)) return false;
    return knownReplies(choice) > 0;
  }

  function knownReplies(choice: CardChoice): number {
    const reported = typeof choice.count === "number" ? choice.count : 0;
    if (reported > 0) return reported;
    const pending = deps.pendingInto(choice.id);
    if (pending > 0) return pending;
    return deps.heldMessages().filter((held) => {
      const thread = (held as { thread?: { is_root?: boolean } }).thread;
      return thread && thread.is_root !== true && deps.threadOf(held) === choice.id;
    }).length;
  }

  function cardView(choice: CardChoice): { mode: string; text: string; cut: CardCut | null; body: () => HTMLElement | null } {
    const words = deps.threadWords(choice);
    if (words.named && words.said) return { mode: "summary", text: `${words.named}: ${words.said}`, cut: null, body: () => null };
    const root = choice.root;
    const entry = root ? cardCutEntry(root) : null;
    if (!entry || !entry.text) return { mode: "name", text: deps.threadName(choice, Infinity), cut: null, body: () => null };
    const { cut, text } = entry;
    const mode = cut === null ? "root" : cardPending(choice) ? "pending" : cut.whole ? "clamped" : "split";
    return { mode, text, cut, body: () => cardHead(root as Message, cut) };
  }

  const rootInward = (choice: CardChoice): boolean => ["summary", "pending", "clamped"].includes(cardView(choice).mode);

  function cardSplit(choice: CardChoice | null): CardCut | null {
    if (!choice) return null;
    const view = cardView(choice);
    return view.mode === "split" ? view.cut : null;
  }

  const summaryWatch = createSummaryWatch({
    setTimer: (run: () => void, ms: number) => deps.setTimer(run, ms),
    clearTimer: (timer: unknown) => deps.clearTimer(timer),
    fired: () => deps.guardQuietly(() => deps.summaryWatchFired())(),
  } as never);

  function watchPendingSummaries(choices: CardChoice[], source: string): void {
    const now = Date.now();
    let owed = Infinity;
    for (const choice of choices) {
      if (!cardPending(choice)) continue;
      const step = SUMMARY_READ_STEPS_MS.map((ms) => (choice.at as number) + ms).find((at) => at > now);
      owed = Math.min(owed, step === undefined ? (choice.at as number) + SUMMARY_WAIT_MS : step);
    }
    (summaryWatch as { owe(source: string, owed: number, now: number): void }).owe(source, owed, now);
  }

  return {
    cardMaxLinesChanged, stepCardMaxLines, quoteReadPercentChanged, stepQuoteReadPercent,
    replyMarksRead, rootOverflowsCard, cardCut, cutForCard, cardHead, continuationOf, cardPending,
    cardView, rootInward, cardSplit, watchPendingSummaries,
  };
}

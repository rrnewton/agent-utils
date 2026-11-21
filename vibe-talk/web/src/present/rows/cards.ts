// What a thread card says and how many lines it takes (`#624 voice-page-modularity`, moved from the
// legacy script into present/rows): the card-line proxy the server counts by (src/threads.rs), a
// thread as every place that names one is handed it, and the facts under its name.

import type { Message, MessageThread, ThreadSummary } from "../../contract/index.ts";
import { namedRecord, timeOf } from "../../core/channels/index.ts";
import { briefAge } from "../time/index.ts";

/** One thread as the page knows it (`threadChoice`). */
interface ThreadChoice {
  id: string;
  summary: ThreadSummary | null;
  named: MessageThread | null;
  root: Message | null;
  count: number | null | undefined;
  at: number;
  started: number;
}

/** The characters one card line is counted at: src/threads.rs `CARD_LINE_CHARS`, the same proxy. */
const CARD_LINE_CHARS = 36;

/** The characters one card line of a list item or a quote is counted at: the card's 36, less its indent. */
const CARD_INDENTED_CHARS = 32;

/**
 * One line of a card's words wrapped as src/threads.rs `underCardlines` wraps it, word by word at
 * `CARD_LINE_CHARS` characters: a word that does not fit begins the next line, and one longer than a
 * whole line is broken across lines. The lines it fills, each with `cut` where it ends inside a word
 * broken so. Word by word because a browser breaks only between words, and a count that ignored that
 * would say fewer lines than the card draws.
 */
function wrapCardLine(line: string, width: number = CARD_LINE_CHARS): Array<{ text: string; cut: boolean }> {
  const lines: Array<{ text: string; cut: boolean }> = [];
  let open = "";
  for (const word of line.split(/\s+/).filter(Boolean)) {
    let chars = [...word];
    if (open && [...open].length + 1 + chars.length <= width) {
      open += ` ${word}`;
      continue;
    }
    if (open) lines.push({ text: open, cut: false });
    while (chars.length > width) {
      lines.push({ text: chars.slice(0, width).join(""), cut: true });
      chars = chars.slice(width);
    }
    open = chars.join("");
  }
  if (open) lines.push({ text: open, cut: false });
  return lines;
}

/** How many card lines `lines` take by the proxy, each wrapped by `wrapCardLine` at `width`. */
function cardProxyLines(lines: readonly string[], width: number = CARD_LINE_CHARS): number {
  return lines.reduce((sum, line) => sum + wrapCardLine(line, width).length, 0);
}

/** `text`'s lines as src/threads.rs `underCardlines` counts them: blank ones dropped, white space closed up. */
const proxyLinesOf = (text: unknown): string[] =>
  String(text).split(/\r?\n/).map((line) => line.replace(/\s+/g, " ").trim()).filter(Boolean);

/** A card's words as text: line by line, each line's white space closed up, blank lines dropped (`#489`). */
const foldedCardText = (text: unknown): string =>
  String(text).split("\n").map((line) => line.replace(/\s+/g, " ").trim()).filter(Boolean).join("\n");

/** The first line of `text` that says anything, cut to `chars` with an ellipsis; "" when none. */
function firstLineOf(text: unknown, chars: number): string {
  const line = String(text || "").split("\n").map((part) => part.trim()).find((part) => part !== "") || "";
  return line.length > chars ? `${line.slice(0, chars - 1)}…` : line;
}

/**
 * The line a thread's title is taken from, as src/threads.rs `title_line` takes it (`#1814`):
 * the first non-empty line that is not part of a leading quote block (lines starting with
 * `>`), else the first quoted line with its quote marks taken off, else "".
 */
function titleLineOf(text: unknown): string {
  let firstQuoted = "";
  for (const part of String(text || "").split("\n").map((line) => line.trim())) {
    if (part === "") continue;
    if (part.startsWith(">")) {
      if (!firstQuoted) firstQuoted = part.replace(/^>+\s*/, "");
      continue;
    }
    return part;
  }
  return firstQuoted;
}

/**
 * One thread as the page knows it: its summary and first message when the store holds them, when
 * it started and was last active, and its reply count. What every place that names a thread is
 * handed, so none of them can reach for a different field.
 */
function threadChoice(id: unknown, summary: ThreadSummary | null = null, root: Message | null = null): ThreadChoice {
  const first = (summary && summary.root) || root || null;
  const record = root && root.thread ? root.thread : null;
  return {
    id: String(id),
    summary,
    // `#528 thread-card-markdown-and-cap`: the name and sentence a root's own record carries, served
    // with every page that brings the root; `threadWords` reads them where the summary says none.
    named: namedRecord(root) || namedRecord(first),
    root: first,
    count: summary ? summary.reply_count : record ? record.reply_count : null,
    at: summary ? timeOf(summary, "updated_at") : timeOf(root, "timestamp"),
    started: first ? timeOf(first, "timestamp") : NaN,
  };
}

/**
 * "13h · 16": how long since the thread was last active, and how many replies it has.
 *
 * Last ACTIVITY rather than when it started, because that is the order every list of threads is in:
 * a start age put "38w" above "2h" in a list sorted newest-activity-first, which reads as unsorted.
 */
function threadFacts(choice: Pick<ThreadChoice, "at" | "started" | "count" | "summary" | "named">): string {
  const age = briefAge(Number.isFinite(choice.at) ? choice.at : choice.started);
  // A count of 0 is never printed: "Thread 0" read as a thread numbered zero, not an empty one.
  const parts = [age, typeof choice.count === "number" && choice.count > 0 ? String(choice.count) : ""];
  // `#1986 vibe-talk-one-page-complete-rows`: who last replied and who is in the thread, when
  // the server counted them onto a record; absent from an older server, nothing is added.
  // Slice 2: the thread list's summary, else the root's own record on the messages page.
  const facts = choice.summary && typeof choice.summary.last_reply_author === "string"
    ? choice.summary : choice.named;
  if (facts && typeof facts.last_reply_author === "string" && facts.last_reply_author !== "") {
    parts.push(`last by ${facts.last_reply_author}`);
  }
  if (facts && Array.isArray(facts.participants) && facts.participants.length > 0) {
    const names = facts.participants.slice(0, 3).map((who) => who.author);
    const rest = facts.participants.length - names.length + (facts.participants_more || 0);
    parts.push(rest > 0 ? `${names.join(", ")} +${rest}` : names.join(", "));
  }
  return parts.filter((part) => part !== "").join(" · ");
}

export { CARD_INDENTED_CHARS, CARD_LINE_CHARS, cardProxyLines, firstLineOf, foldedCardText, proxyLinesOf, threadChoice, threadFacts, titleLineOf, wrapCardLine };

/** Longest filed thread slug, in characters: src/summarize/threads.rs `MAX_SLUG_CHARS`. */
export const MAX_THREAD_SLUG_CHARS = 40;

/**
 * Whether `name` is a usable filed thread slug, held to the server's shape
 * (src/summarize/threads.rs `valid_thread_slug`, `#1819 vibe-talk-thread-button-shows-slug`):
 * two to four words of lowercase ASCII letters and digits, joined by single
 * hyphens, at most {@link MAX_THREAD_SLUG_CHARS} characters. The same filed
 * name #1796 joins onto `:thread:` refs; a name that fails is no slug.
 */
export function validThreadSlug(name: unknown): boolean {
  if (typeof name !== "string") return false;
  const slug = name.trim();
  if (slug.length === 0 || slug.length > MAX_THREAD_SLUG_CHARS) return false;
  const words = slug.split("-");
  return words.length >= 2 && words.length <= 4
    && words.every((word) => word !== "" && [...word].every((ch) => (ch >= "a" && ch <= "z") || (ch >= "0" && ch <= "9")));
}

/** The first of `names` that is a filed thread slug, trimmed; "" when none is. */
export function filedThreadSlug(...names: unknown[]): string {
  for (const name of names) {
    if (validThreadSlug(name)) return (name as string).trim();
  }
  return "";
}

export type { ThreadChoice };

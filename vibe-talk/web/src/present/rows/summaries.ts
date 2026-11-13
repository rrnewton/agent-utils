// What the summaries say (`#49 cached-summaries`, `#564 message-summary-toggle`; `#624
// voice-page-modularity`, moved from the legacy script into present/rows): the marks on a summarised
// row, the chip's two labels, the standing note at the head of the channel view, and the counted pill
// for failures. What is held, and asked, is web/src/core/channels's (`createSummaries`).

/** What a summarised row shows before its answer arrives, and what marks it as not the message. */
const SUMMARY_MARK = "summary";
const SUMMARY_WAITING = "summarising…";
/**
 * ...and what a row says when the summary did not come.
 *
 * IN WORDS, and since `#564 message-summary-toggle` ONLY in words: small and muted, the row's own
 * surface, read state and controls untouched (web/voice.css).
 */
const SUMMARY_FAILED_MARK = "summary failed";

/** The chip over the list, off and on. */
const SUMMARY_MODE_OFF = "Summaries";
const SUMMARY_MODE_ON = "Summaries on";

/** The median, which is what a reader wants: one slow first call must not describe the rest. */
function typicalSummaryMs(times: readonly number[]): number | null {
  if (times.length === 0) {
    return null;
  }
  const sorted = [...times].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] as number;
}

/**
 * The standing sentence at the head of the channel view while the mode is on: why this server cannot
 * summarise (`unavailable`), or what summaries are, who makes them (`backend`) and how long they take.
 */
function summaryNoteWords(unavailable: string, backend: string, times: readonly number[]): string {
  // THE DEPLOYMENT-LEVEL VERDICT, and it replaces the ordinary sentence rather than trailing it.
  // "Collapsed messages show a summary" is simply untrue on a server that cannot make one, and a
  // note that says both things reads as a page that has not noticed. The pill that announced this
  // is gone within six seconds; this is what is still on screen a minute later.
  if (unavailable) {
    // The server's sentence, punctuated if it did not punctuate itself, so the two statements do
    // not run into one another as a single unreadable line.
    const said = /[.!?]$/.test(unavailable)
      ? unavailable
      : `${unavailable}.`;
    return `Summaries are unavailable on this server: ${said} ` +
      "Collapsed messages keep their opening lines.";
  }
  const base = "Collapsed messages show a summary instead of their opening lines. Tap More for " +
    "the message itself.";
  const parts = [base];
  if (backend) {
    parts.push(`Summaries by ${backend}.`);
  }
  const typical = typicalSummaryMs(times);
  if (typical !== null) {
    const seconds = (typical / 1000).toFixed(1);
    // The COUNT as well as the time, because a median over two samples is not a median.
    parts.push(`Typically ${seconds}s (${times.length} measured).`);
  }
  return parts.join(" ");
}

/** The pill for `count` rows that could not be summarised, the latest because `why`. */
function summaryFailureWords(count: number, why: string): string {
  const many = count === 1 ? "one message" : `${count} messages`;
  return `${many} could not be summarised: ${why}`;
}

// What the page and the tests use of it.
export {
  SUMMARY_FAILED_MARK,
  SUMMARY_MARK,
  SUMMARY_MODE_OFF,
  SUMMARY_MODE_ON,
  SUMMARY_WAITING,
  summaryFailureWords,
  summaryNoteWords,
  typicalSummaryMs,
};

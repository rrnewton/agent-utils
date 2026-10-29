// What a slow or failing chat service means, in words (`#195 send-resilience`; `#624
// voice-page-modularity`, moved from the legacy script). The server's sentence for a provider
// failure is a diagnosis, right in a log and wrong on a phone; these say what it means instead.

import { sentenceStart } from "../../foundation/index.ts";

/** A failed request's facts as these read them: an {@link ApiRequestError}, or whatever was thrown. */
export interface FailureFacts {
  readonly message?: unknown;
  readonly network?: unknown;
  readonly timedOut?: unknown;
  readonly status?: unknown;
  readonly detail?: unknown;
}

/** Wording that marks a failure as a service that did not answer in time. */
export const SLOW_WORDS = /timed?[ -]?out|timeout|deadline/i;

/**
 * The provider's own words inside a server detail such as `X returned HTTP 502: {"message":"…"}`,
 * or the detail itself when it is already prose.
 */
export function providerWords(detail: unknown): string {
  const text = String(detail || "").trim();
  const brace = text.indexOf("{");
  if (brace < 0) return text;
  try {
    const inner = JSON.parse(text.slice(brace)) as { message?: unknown; detail?: unknown; error?: unknown } | null;
    const said = inner && [inner.message, inner.detail, inner.error].find((value) =>
      typeof value === "string" && value.trim());
    if (said) return (said as string).trim();
  } catch (_error) {
    // A provider body the server cut short is not JSON any more; its message may still be whole.
    // `\x22` is a double quote, spelled out so the page suite's scan for string literals does
    // not read one opening here.
    const quoted = /\x22(?:message|detail)\x22\s*:\s*\x22((?:[^\x22\\]|\\.)+)\x22/.exec(text);
    if (quoted) return quoted[1]!.replace(/\\(.)/g, "$1");
  }
  return text.slice(0, brace).replace(/[:\s]+$/, "") || text;
}

/** Whether a failure is a service that did not answer in time, rather than one that said no. */
export function isSlowFailure(error: FailureFacts | null | undefined): boolean {
  if (!error) return false;
  if (error.timedOut) return true;
  const status = Number(error.status) || 0;
  return status === 504 || status === 408 || (status >= 500 && SLOW_WORDS.test(String(error.detail || "")));
}

/**
 * A failed request to `service`, in words for the reader. Only a 5xx is reworded: anything else is
 * this server's own sentence or the page's, and is returned as it was.
 */
export function readableFailure(error: FailureFacts | null | undefined, service: string): string {
  if (!error) return "";
  const status = Number(error.status) || 0;
  if (error.network || error.timedOut || status < 500) return String(error.message || "");
  const name = sentenceStart(service);
  if (isSlowFailure(error)) return `${name} is slow to answer right now.`;
  const said = providerWords(error.detail);
  return said ? `${name} could not answer: ${said}` : `${name} could not answer (HTTP ${status}).`;
}

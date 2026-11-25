// Why a document did not load, in words, and who said so (`#1980
// vibe-talk-docs-private-github-load`; `#2241 vibe-talk-document-load-502-readable`).
//
// Shared by the reader and the list, so a failed card and a failed reader say the
// same thing. Pure: the wording is testable without a page.

/** Who answered the failed load: what the reader's sentence has to tell apart (`#2241`). */
export type DocumentFailureSource =
  /** vibe-talk itself answered, with its own error and cause. */
  | "vibe-talk"
  /** Something in front of vibe-talk (a proxy, a tunnel) answered instead of it. */
  | "front-end"
  /** Nothing answered: the page could not reach vibe-talk at all. */
  | "network"
  /** The page could not tell. */
  | "unknown";

/** One failed load, in the words and with the ways out the reader shows (`#1980`, `#2241`). */
export interface DocumentLoadFailure {
  /** The specific reason, as a sentence. Never a bare "could not load", never a raw parse error. */
  reason: string;
  /** The document's own safe http(s) address, or null where none is known. */
  href: string | null;
  /** What the way-out link says: GitHub by name for a github.com address. */
  linkLabel: string;
  /** Who answered the failure (`#2241`). */
  source: DocumentFailureSource;
}

/** Characters that may not appear in an address the title may link to (render.ts's discipline). */
const UNSAFE_URL_CHARS = /[\s"<>\u0000-\u001f\u007f]/;

/**
 * The `href` for a source link, or null where the address is not one the page
 * will follow: `http://` and `https://` (in any casing) only, with no
 * whitespace or control characters. Anything else — `javascript:` included —
 * links nowhere.
 */
export function safeSourceHref(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "" || UNSAFE_URL_CHARS.test(trimmed)) return null;
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://")) return trimmed;
  return null;
}

/** The facts of a failed request the wording reads (an `ApiRequestError`, or whatever was thrown). */
interface FailureRecord {
  message?: unknown;
  detail?: unknown;
  code?: unknown;
  status?: unknown;
  network?: unknown;
  gatewayStatus?: unknown;
}

/**
 * The HTTP status of a failed answer that was NOT vibe-talk's own, or null
 * (`#2241`). The API client keeps a body that is not JSON as `gatewayStatus`
 * (a proxy's or tunnel's error page), and a JSON body outside vibe-talk's
 * error contract as code `error` with no detail (a gateway's own JSON). Every
 * answer vibe-talk itself gives under `/api/`, refusals and failures alike,
 * is its contract JSON with a code of its own.
 */
export function frontEndStatus(error: unknown): number | null {
  const record = (typeof error === "object" && error !== null ? error : {}) as FailureRecord;
  const gateway = typeof record.gatewayStatus === "number" ? record.gatewayStatus : 0;
  if (gateway >= 400) return gateway;
  // A body that was not JSON on an answer the client did not count as failed
  // (a captive portal's 200 page): the client's sentence carries its status.
  const unparsed = typeof record.message === "string" ? /returned non-JSON \(HTTP (\d+)\)/.exec(record.message) : null;
  if (unparsed) return Number(unparsed[1]);
  const status = typeof record.status === "number" ? record.status : 0;
  const detail = typeof record.detail === "string" ? record.detail.trim() : "";
  if (status >= 400 && record.code === "error" && detail === "") return status;
  return null;
}

/** The host of a safe address, for naming where the document lives; null where none parses. */
function hostOf(href: string | null): string | null {
  if (href === null) return null;
  try {
    const host = new URL(href).hostname;
    return host === "" ? null : host;
  } catch {
    return null;
  }
}

/**
 * Why one load failed, from the API error's own facts: whose answer it was
 * and its specific reason (`#1980`, `#2241`). vibe-talk's own error names
 * vibe-talk and keeps its cause; an answer from whatever fronts vibe-talk (a
 * proxy's 502 page, a tunnel timing out) says so, with the document's host
 * and the status, instead of the raw "returned non-JSON" parse error; a
 * network failure, a rate limit, a refusal and an unindexed key are each
 * named. The result always names a specific cause.
 */
export function documentLoadFailure(error: unknown, url?: unknown): DocumentLoadFailure {
  const record = (typeof error === "object" && error !== null ? error : {}) as FailureRecord;
  const detail = typeof record.detail === "string" ? record.detail.trim() : "";
  const message = typeof record.message === "string" ? record.message : String(error ?? "");
  const code = typeof record.code === "string" ? record.code : "";
  const status = typeof record.status === "number" ? record.status : null;
  // The address is the server's own data (the document row), never one the
  // page invents from the key: the page hardcodes no off-origin destination.
  const href = safeSourceHref(url);
  const host = hostOf(href);
  const frontStatus = frontEndStatus(error);
  let reason: string;
  let source: DocumentFailureSource = "vibe-talk";
  if (record.network === true || /could not reach vibe-talk/i.test(message)) {
    reason = "Could not reach vibe-talk (network) while loading this document. Check the connection and try again.";
    source = "network";
  } else if (frontStatus !== null) {
    const page = record.code === "error" ? "its own answer" : "its own error page";
    reason = `Could not load this document${host === null ? "" : ` from ${host}`}: the proxy or tunnel ` +
      `in front of vibe-talk answered HTTP ${frontStatus} with ${page}, not vibe-talk's answer. ` +
      "vibe-talk may have been too slow fetching it, or may not be running. Try again.";
    source = "front-end";
  } else if (detail !== "") {
    // vibe-talk's own cause, saying it is vibe-talk's (`#2241`); a cause
    // that already names vibe-talk (`#1997` helper failures, `#1980` no
    // access) stands as the server wrote it.
    reason = /^vibe-talk\b/i.test(detail) ? detail : `vibe-talk could not fetch this document: ${detail}`;
  } else if (code === "unknown_document" || message.includes("unknown_document")) {
    reason = "This document is not in vibe-talk's index yet, and it could not be fetched.";
  } else if (status === 429 || /rate limit/i.test(message)) {
    reason = "GitHub rate limit: GitHub asked vibe-talk to pause. Try again later.";
  } else if (status === 401 || status === 403 || /\bHTTP (401|403)\b/.test(message) || code === "forbidden") {
    reason = "vibe-talk has no access to this document: the request was refused (private repository or a credential GitHub refused).";
  } else if (status === 404 || /\bHTTP 404\b/.test(message)) {
    reason = "Not found: GitHub has no such document at this address, or the repository is private and vibe-talk has no access to it.";
  } else if (/could not reach GitHub/i.test(message)) {
    reason = message.replace(/^HTTP \d+ [^:]+:\s*/, "");
  } else if (message !== "") {
    reason = message;
    source = "unknown";
  } else {
    reason = "vibe-talk could not fetch this document for an unknown reason.";
    source = "unknown";
  }
  let linkLabel = "Open source";
  if (host === "github.com" || host === "gist.github.com") linkLabel = "Open on GitHub";
  return { reason, href, linkLabel, source };
}

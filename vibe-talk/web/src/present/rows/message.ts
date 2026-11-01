// What a message's row is drawn from (`#624 voice-page-modularity`, moved from the legacy script into
// present/rows): its raw text and rendered body, whether its text has anything in it to see, and the
// signature that says when a kept row must be drawn again.

import type { Message } from "../../contract/index.ts";

/** A message's rendered body, or "" when it has none and is drawn from its text. */
const renderedHtmlOf = (message: Pick<Message, "content_html"> | null | undefined): string =>
  message && typeof message.content_html === "string" ? message.content_html : "";

/** A message's raw text, as written. */
const rawTextOf = (message: Pick<Message, "content"> | null | undefined): string =>
  String(message && message.content !== null && message.content !== undefined ? message.content : "");

/** Text with nothing in it to see: white space, and the zero-width characters a body can carry. */
const BLANK_TEXT = /^[\s\u200b-\u200d\u2060]*$/u;

/**
 * What a message's row draws from it beyond its id, as a signature: its words and their rendering (by
 * length, and the edit the provider reported) and its reactions. A row kept from the last pass
 * (`renderSideCards`) is drawn again when this changes: an edit or a reaction since. `#532 shift-o-latency`.
 */
function messageLook(message: Message): string {
  const words = `${rawTextOf(message).length}:${renderedHtmlOf(message).length}`;
  return `${String(message.id)}|${words}|${message.edited_timestamp || ""}|${JSON.stringify(message.reactions || null)}`;
}

export { BLANK_TEXT, messageLook, rawTextOf, renderedHtmlOf };

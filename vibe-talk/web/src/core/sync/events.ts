// Reading a Server-Sent Events stream into frames (`#337 voice-relay-silent` and the channel stream;
// `#624 voice-page-modularity`, moved from the legacy script; plan step 14). Why the page reads the
// stream by hand rather than with `EventSource` is told where the legacy script opens it.

/** One SSE frame, as fields. */
export interface EventFrame {
  id: string | null;
  event: string;
  data: string;
}

/** A reader of the response body's bytes. */
export interface ByteReader {
  read(): Promise<{ readonly done: boolean; readonly value?: unknown }>;
  cancel?(): unknown;
}

/** Turns bytes into text, holding a character split across chunks while `stream` is set (a `TextDecoder`). */
export interface TextChunkDecoder {
  decode(bytes: unknown, options: { stream: boolean }): string;
}

/**
 * Hand each Server-Sent Event of `reader` to `onFrame` until the response ends, or `current()` says
 * the stream it belongs to was stopped. Answers `reset` when a frame's handler said the server reset
 * the stream, and `ended` otherwise.
 */
export async function readEventStream(
  reader: ByteReader,
  current: () => boolean,
  onFrame: (frame: EventFrame) => string | void,
  decoder: TextChunkDecoder,
): Promise<"reset" | "ended"> {
  let buffer = "";
  let reset = false;
  for (;;) {
    const chunk = await reader.read();
    if (!current()) {
      // Deliberately not awaited and deliberately not fatal: a reader that will not cancel must
      // not keep this loop alive, and the check above has already stopped it mattering.
      if (typeof reader.cancel === "function") {
        Promise.resolve(reader.cancel()).catch(() => {});
      }
      return "ended";
    }
    if (chunk.done) {
      return reset ? "reset" : "ended";
    }
    buffer += decoder.decode(chunk.value, { stream: true });
    // An SSE event ends at a blank line. Anything after the last one is a partial frame and stays
    // in the buffer — a chunk boundary falls wherever the network puts it, not where a message
    // ends.
    let at = buffer.indexOf("\n\n");
    while (at >= 0) {
      if (onFrame(parseEventFrame(buffer.slice(0, at))) === "reset") reset = true;
      buffer = buffer.slice(at + 2);
      at = buffer.indexOf("\n\n");
    }
  }
}

/**
 * One SSE frame, as fields.
 *
 * Lines beginning with a colon are comments — which is what a keep-alive is — and produce a frame
 * with no data, so the caller ignores it without needing to know that.
 */
export function parseEventFrame(text: string): EventFrame {
  const frame: EventFrame = { id: null, event: "message", data: "" };
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    if (!line || line.startsWith(":")) {
      continue;
    }
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) {
      value = value.slice(1);
    }
    if (field === "id") {
      frame.id = value;
    } else if (field === "event") {
      frame.event = value;
    } else if (field === "data") {
      frame.data = frame.data ? `${frame.data}\n${value}` : value;
    }
  }
  return frame;
}

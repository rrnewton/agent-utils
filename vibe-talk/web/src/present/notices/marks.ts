// What the status line says of a message marked read here and in its chat service (`#624
// voice-page-modularity`, moved from the legacy script into present/notices).

/** What marking a message read in its chat service came to (`markReadInService`). */
interface ServiceReadMark {
  readonly service: string;
  readonly marked: boolean;
  readonly why: string;
  /** whether the chat service's marker was already through the message before it was asked */
  readonly already?: boolean;
}

/**
 * The status line's words for a message marked read here, by what its chat service did
 * (`markReadInService`), with `also` — Shift+E's ", and collapsed" — said of the message after where
 * it was marked.
 */
function serviceReadWords(mark: ServiceReadMark, also = ""): string {
  if (mark.marked) return `Marked read here and in ${mark.service}${also}.`;
  return `Marked read here, not in ${mark.service}${also}${mark.why ? `: ${mark.why}` : "."}`;
}

/**
 * `serviceReadWords` for a message that was Done here before Shift+E was pressed on it
 * (`readAndCollapse`): "already" said of here, and of the chat service only where the server answered
 * that its marker was through the message before it was asked. `#295 shift-e-read-collapse`.
 */
function doneReadWords(mark: ServiceReadMark, also = ""): string {
  if (mark.marked && mark.already) return `Already read here and in ${mark.service}${also}.`;
  if (mark.marked) return `Already read here; now marked read in ${mark.service} too${also}.`;
  return `Already read here, not in ${mark.service}${also}${mark.why ? `: ${mark.why}` : "."}`;
}

/**
 * What the status line says of Shift+E's read the moment it is pressed, before the server or the chat service
 * has answered anything (`#892 voice-page-optimistic-done`): marked read here, or already read here when
 * `wasDone`, with `also`. The service's half is said over it once answered (`serviceReadWords`,
 * `doneReadWords`), while nothing else has been said since.
 */
function localReadWords(wasDone: boolean, also = ""): string {
  return wasDone ? `Already read here${also}.` : `Marked read here${also}.`;
}

export { doneReadWords, localReadWords, serviceReadWords };
export type { ServiceReadMark };

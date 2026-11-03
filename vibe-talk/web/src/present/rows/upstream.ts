// A message's own addresses in the service it came from (`#408 copy-upstream-link`, `#409
// open-upstream-link`, `#481 slack-app-deep-link`; `#624 voice-page-modularity`, moved from the legacy
// script into present/rows): the web address and the app address the server formed and the message carries,
// each read only where it has the one shape its sink may take. The ⋯ menu's "Open in …" link and the l
// key's copy both take the address from here, so the two are offered together and never disagree about it.

import { HTTP_ADDRESS } from "../../foundation/index.ts";

/** A message, as far as its addresses are read. */
interface Addressed {
  readonly upstream_url?: unknown;
  readonly upstream_app_url?: unknown;
}

/**
 * A message's own address in the service it came from, or null where none came back.
 *
 * The address lands in an href, the one sink that is not plain text, so nothing is read here that
 * `HTTP_ADDRESS` has not passed: the server forms only https addresses, and one of any other shape is no
 * address to offer, to open or to copy.
 */
function upstreamUrlOf(message: unknown): string | null {
  const held = message as Addressed | null | undefined;
  const url = held && typeof held.upstream_url === "string" ? held.upstream_url : "";
  return /^https:\/\//i.test(url) && !/\s/.test(url) && HTTP_ADDRESS.test(url) ? url : null;
}

/**
 * A message's address in its service's own app, as the server formed it (`slack://` for Slack), or null.
 * The counterpart of `upstreamUrlOf` with that sink's own shape: only the one scheme and the parts a
 * Slack app address is made of pass, so nothing a message carries can put another scheme in the href.
 */
function upstreamAppUrlOf(message: unknown): string | null {
  const held = message as Addressed | null | undefined;
  const url = held && typeof held.upstream_app_url === "string" ? held.upstream_app_url : "";
  return /^slack:\/\/channel\?team=T[A-Z0-9]+&id=[A-Z0-9]+(&message=[0-9.]+)?(&thread_ts=[0-9.]+)?$/.test(url) ? url : null;
}

export { upstreamAppUrlOf, upstreamUrlOf };

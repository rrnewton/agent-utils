// The identities a link names (`#258 deep-links`, `#433 in-app-links`): reading and writing the
// `m` value of an address and the `data-msg` of a drawn link. Moved here from web/voice.js unchanged
// but for its types (`#624 voice-page-modularity`).

/** A message as a link's `m` names it: its service, channel, id, and thread (null in the main channel). */
export interface DeepLinkRef {
  readonly provider: string;
  readonly channel: string;
  readonly message: string;
  readonly thread: string | null;
}

/** What a drawn link's `data-msg` names; the message is null for a link to a channel. */
export interface MessageLinkRef {
  readonly service: string;
  readonly channel: string;
  readonly message: string | null;
  readonly thread: string | null;
}

/** The longest query value read from a link: an id padded past this is not an id. */
const DEEP_LINK_PART_MAX = 512;

/** The message a link's `m` names, its parts unescaped; null for a shape no link of ours makes. */
function deepLinkRefFrom(text: unknown): DeepLinkRef | null {
  const parts = String(text || "").split(":");
  if (parts.length < 3 || parts.length > 4) return null;
  try {
    const decoded = parts.map((part) => decodeURIComponent(part));
    if (decoded.some((part) => !part || part.length > DEEP_LINK_PART_MAX)) return null;
    return { provider: decoded[0]!.toLowerCase(), channel: decoded[1]!, message: decoded[2]!, thread: decoded[3] || null };
  } catch (_error) {
    return null;
  }
}

/** A ref as the `m` value carries it: each part escaped, then joined. */
function deepLinkRefText(ref: DeepLinkRef): string {
  const parts = [ref.provider, ref.channel, ref.message, ...(ref.thread ? [ref.thread] : [])];
  return parts.map((part) => encodeURIComponent(part)).join(":");
}

/** What a link's `data-msg` names: `service:channel:message:thread`, each escaped; null for any other shape. */
function messageLinkRef(text: unknown): MessageLinkRef | null {
  const parts = String(text || "").split(":");
  if (parts.length !== 4) return null;
  try {
    const [service, channel, message, thread] = parts.map((part) => decodeURIComponent(part)) as [string, string, string, string];
    if (!service || !channel || [channel, message, thread].some((part) => part.length > DEEP_LINK_PART_MAX)) return null;
    return { service, channel, message: message || null, thread: thread || null };
  } catch (_error) {
    return null;
  }
}

// What the page and the tests use of it.
export { DEEP_LINK_PART_MAX, deepLinkRefFrom, deepLinkRefText, messageLinkRef };

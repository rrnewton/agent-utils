// Where the reader stands on a channel, as a record (`#423 back-key-consistency`): which layout and
// view, which thread, and what a thread was opened from. The page's history of them and the
// showing of one are the page's; what tells two apart and which close onto another are here.
// Moved here from web/voice.js unchanged but for its types (`#624 voice-page-modularity`).

/** A place on a channel's main screen. */
export interface ChannelPlace {
  screen: "main";
  channel: string;
  layout: "one" | "list" | "two";
  view: string | null;
  thread: string | null;
  origin: string | null;
  side: string | null;
}

/** Any place: a channel's, or Settings or Help over the channel place they were opened from. */
export type Place = ChannelPlace | {
  screen: "settings" | "help";
  channel: string;
  under: ChannelPlace | null;
  from: string | null;
  entry: string | null;
};

/** What tells one place from another. A thread's origin says where its own Back goes, not where it is. */
function visitKey(place: Place | null | undefined): string {
  if (!place) return "";
  if (place.screen !== "main") return `${place.screen}:${place.from || ""}:${visitKey(place.under)}`;
  return [place.layout, place.view || "", place.thread || "", place.side || ""].join(":");
}

/**
 * Whether `place` is open over another, something Esc closes onto the place under it: Settings, Help, the
 * thread list, a thread opened from it, a thread on the right.
 */
function placeIsOver(place: Place): boolean {
  if (place.screen !== "main") return true;
  return place.layout === "list" || (place.layout === "two" && place.side !== null) ||
    (place.layout === "one" && place.view === "thread" && place.origin === "list");
}

// What the page and the tests use of it.
export { placeIsOver, visitKey };

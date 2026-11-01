// How long ago, in a picker's few letters (`#624 voice-page-modularity`, moved from the legacy script
// into present/time).

/**
 * How long ago, as briefly as a picker option allows — and ONE SCALE wherever the page gives a
 * thread's age: the selector, the heading over an open thread, and the thread list's cards.
 * `#194 thread-picker-polish`: the owner read "42h" in the selector and asked for days and hours.
 *
 *   under a minute     now
 *   under an hour      59m
 *   under a day        23h
 *   under ten days     1d18h, and a whole day drops its "0h": 3d
 *   under four weeks   12d    — by then the hours no longer tell two threads apart
 *   under a year       6w
 *   beyond             2y
 *
 * Every step truncates and never rounds, so an age reads as the last whole unit it has passed:
 * 42 hours is 1d18h, never 2d, and 59 minutes is 59m, never 1h.
 */
function briefAge(atMs: number): string {
  if (!Number.isFinite(atMs)) return "";
  const minutes = Math.max(0, Math.floor((Date.now() - atMs) / 60000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 10) return hours % 24 === 0 ? `${days}d` : `${days}d${hours % 24}h`;
  if (days < 28) return `${days}d`;
  return days < 365 ? `${Math.floor(days / 7)}w` : `${Math.floor(days / 365)}y`;
}

export { briefAge };

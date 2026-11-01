// Counts, sizes and durations in the page's fixed English (`#624 voice-page-modularity`, moved from the
// legacy script into present/notices).

/** A count with its thousands marked, in the page's fixed English: 1,200. */
const groupedCount = (n: number): string => String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ",");

/** `n` and its noun, singular for exactly one. */
const nounCount = (n: number, noun: string): string => `${groupedCount(n)} ${noun}${n === 1 ? "" : "s"}`;

/** The server's megabyte, as its limit counts one. */
const MEBIBYTE = 1024 * 1024;

/** Bytes in the unit the limit beside them uses: megabytes, or kilobytes below one. */
function cacheSize(bytes: number): string {
  if (bytes >= MEBIBYTE) return `${(bytes / MEBIBYTE).toFixed(1)} MB`;
  return `${bytes === 0 ? 0 : Math.max(1, Math.ceil(bytes / 1024))} KB`;
}

/** One duration for the tap readout: `120 ms`, `1.9 s`. `#621 send-double-check`. */
function fmtDuration(ms: unknown): string {
  if (ms === null || ms === undefined || !Number.isFinite(Number(ms))) return "—";
  const value = Math.max(0, Math.round(Number(ms)));
  return value < 1000 ? `${value} ms` : `${(value / 1000).toFixed(1)} s`;
}

export { MEBIBYTE, cacheSize, fmtDuration, groupedCount, nounCount };

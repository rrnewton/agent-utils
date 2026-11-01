// A message's time as the reader's clock says it (`#624 voice-page-modularity`, moved from the legacy
// script into present/time): the row's hour and minute, the details sheet's whole instant, and the
// titles that add the day.

import { dayLabel, dayTitle } from "./days.ts";

/** What a time is read from: a message's ISO timestamp, or failing that the words the server spoke it in. */
interface Timed {
  readonly timestamp?: unknown;
  readonly spoken_time?: unknown;
}

/** Wall-clock, as two numbers. A surface used as a debugging record has to say when. */
function stamp(atMs?: number): string {
  const now = atMs === undefined ? new Date() : new Date(atMs);
  const pad = (n: number): string => String(n).padStart(2, "0");
  return `${pad(now.getHours())}:${pad(now.getMinutes())}`;
}

/**
 * The clock the READER is standing in, not the one the server was configured with.
 *
 * `#52 operator-timezone` had the server convert once, into `server.timezone`, and the page simply
 * printed what it was handed. That is right for the voice agent — it has to SPEAK a time and
 * cannot ask a browser — and wrong for a phone: `server.timezone` defaults to UTC, so an operator
 * who never set it reads every message in UTC while holding a device that knows perfectly well
 * what time it is. The browser's own zone is not a guess; it is the answer.
 *
 * `spoken_time` stays as the fallback for a message with no parseable timestamp, so a server that
 * sends only the spoken form still shows something rather than nothing.
 */
function messageDate(message: Timed): Date | null {
  const raw = String(message.timestamp || "");
  if (!raw) {
    return null;
  }
  const at = new Date(raw);
  return Number.isNaN(at.getTime()) ? null : at;
}

/** `13:40` — the hour and minute, which is all a row has room for. */
function shortLocalTime(message: Timed): string {
  const at = messageDate(message);
  if (at === null) {
    return String(message.spoken_time || "");
  }
  return shortTimeFormat().format(at);
}

/**
 * The formatter `shortLocalTime` uses: what `toLocaleTimeString([], { hour, minute })` makes on every
 * call, made once. `#226 page-energy-profile`: every row asked for its own on every redraw, which was
 * a twentieth of the page's work in live traffic. Made again when the device's offset from UTC
 * moves — the reader travelled, or the clocks changed — since a formatter keeps the zone it was made
 * in.
 */
let shortTimeFormatter: { offset: number; format: Intl.DateTimeFormat } | null = null;

function shortTimeFormat(): Intl.DateTimeFormat {
  const offset = new Date().getTimezoneOffset();
  if (shortTimeFormatter === null || shortTimeFormatter.offset !== offset) {
    shortTimeFormatter = { offset, format: new Intl.DateTimeFormat([], { hour: "2-digit", minute: "2-digit" }) };
  }
  return shortTimeFormatter.format;
}

/**
 * What hovering an edited mark says: when the message was last edited, in the words its time's own
 * title uses — "Edited Yesterday, Oct 11, Sun, 5:25 PM".
 */
const editedTitle = (at: Date): string => `Edited ${dayTitle(at, shortTimeFormat().format(at))}`;

/**
 * The whole truth, for the details sheet: day, time and zone, all local — "Yesterday, Oct 11, Sun,
 * 05:25 PM PDT". The day is `dayLabel`'s, so the sheet a phone opens with a press-and-hold says it in
 * the words a desk's tooltip uses (`#260 timestamp-hover`), and it is worded as the sheet opens.
 */
function fullLocalTime(message: Timed): string {
  const at = messageDate(message);
  if (at === null) {
    return String(message.spoken_time || "unknown");
  }
  return fullLocalAt(at);
}

/** `at` as the details sheet says an instant: "Yesterday, Oct 11, Sun, 05:25 PM PDT". */
function fullLocalAt(at: Date): string {
  // Explicit components, NOT `timeStyle`. ECMA-402 forbids combining that with an individual field
  // such as `timeZoneName`, and a browser answers that with a TypeError rather than by ignoring the
  // option — so the sheet would have thrown instead of opening.
  return `${dayLabel(at)}, ${at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZoneName: "short" })}`;
}

export { editedTitle, fullLocalAt, fullLocalTime, messageDate, shortLocalTime, shortTimeFormat, stamp };
export type { Timed };

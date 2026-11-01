// Which day a message was sent, in words (`#624 voice-page-modularity`, moved from the legacy script
// into present/time, a presentation decision with no DOM).
//
// `#260 timestamp-hover`. The owner: "On an old thread a message says 5:25 PM but it is 3:56 PM now,
// so it must be yesterday." A row's time is the hour and minute and nothing else, because that is all
// a row has room for, and on its own it cannot say which day. Hovering it now says: "Yesterday,
// Oct 11, Sun, 5:25 PM".
//
// `dayLabel` is THE ONE DAY FORMATTER on this page. The tooltip over every message's time and the
// details sheet ask it, and so do `#250 date-separators`' lines between days in a list, rather than
// make a second wording that drifts from this one:
//
//   the day the reader is in     Today, Oct 12, Mon
//   the day before it            Yesterday, Oct 11, Sun
//   any other day this year      Oct 9, Fri
//   a day in another year        Oct 9, 2025, Thu
//
// The year appears only when it is not the reader's current year, and that holds for Today and
// Yesterday too: on New Year's Day the evening before is "Yesterday, Dec 31, 2025, Wed".
//
//   * CALENDAR DAYS, NOT SPANS OF 24 HOURS. "Yesterday" is the date before today's, compared field
//     by field in the reader's own zone, so a day that is 23 or 25 hours long because the clocks
//     changed in it is still exactly one day. Thirty hours ago can be yesterday or the day before,
//     and only the calendar knows which.
//   * THE DATE GETTERS, NOT AN `Intl` FORMATTER. Every row's tooltip asks this on every redraw, and
//     `#226 page-energy-profile` found a formatter made per row to be a twentieth of the page's work.
//     The getters already answer in the device's zone and follow it when it moves.
//   * FIXED ENGLISH NAMES. Every other word this page shows is English, and "Yesterday, 11 oct., lun."
//     beside them would be a sentence in two languages.

const DAY_MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const DAY_WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** Whether two instants fall on the same calendar date in the reader's zone. */
const sameLocalDay = (a: Date, b: Date): boolean =>
  a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();

/**
 * The day `when` falls on, as the reader says it standing at `now`: "Today, Oct 12, Mon",
 * "Yesterday, Oct 11, Sun", "Oct 9, Fri", "Oct 9, 2025, Thu". Both are Dates, read in local time.
 * See the section note above: this is the only place a day is worded.
 */
function dayLabel(when: Date, now: Date = new Date()): string {
  const year = when.getFullYear() === now.getFullYear() ? "" : `, ${when.getFullYear()}`;
  const day = `${DAY_MONTHS[when.getMonth()]} ${when.getDate()}${year}, ${DAY_WEEKDAYS[when.getDay()]}`;
  if (sameLocalDay(when, now)) return `Today, ${day}`;
  // Built from the date's own fields, so the constructor steps back a calendar day however long that
  // day was. Noon, because in a zone whose clocks change at midnight that midnight may not exist.
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1, 12);
  return sameLocalDay(when, yesterday) ? `Yesterday, ${day}` : day;
}

/**
 * What hovering a message's time says: its day, then the time exactly as it is shown —
 * "Yesterday, Oct 11, Sun, 5:25 PM".
 */
const dayTitle = (at: Date, shown: string): string => `${dayLabel(at)}, ${shown}`;

// --- which day ------------------------------------------------------------------------------------
//
// `dayLabel` is the page's one wording of a single day, in the owner's words, and a day rule says
// exactly what it does. It is defined once, with `DAY_MONTHS`, `DAY_WEEKDAYS` and `sameLocalDay`,
// beside the time's tooltip (`#260 timestamp-hover`), which says a day in the same words.

/** A local calendar day as a number that sorts as the days do: yyyymmdd. */
const dayKeyOf = (date: Date): number => date.getFullYear() * 10000 + (date.getMonth() + 1) * 100 + date.getDate();

/** Now, and today, yesterday and this year on this device's calendar, read once for a pass. */
interface DayClock {
  readonly now: Date;
  readonly today: number;
  readonly yesterday: number;
  readonly year: number;
}

/** Now, and today, yesterday and this year on this device's calendar, read once for a pass. */
function dayClock(): DayClock {
  const now = new Date();
  return {
    now,
    today: dayKeyOf(now),
    yesterday: dayKeyOf(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1, 12)),
    year: now.getFullYear(),
  };
}

/** A day key's year, month (from 0) and day of the month. */
const dayParts = (key: number): { year: number; month: number; day: number } =>
  ({ year: Math.floor(key / 10000), month: Math.floor(key / 100) % 100 - 1, day: key % 100 });

/** One day, as a rule says it: `dayLabel`'s words for the day's noon, which every zone's calendar has. */
function dayWords(key: number, clock: DayClock): string {
  const { year, month, day } = dayParts(key);
  return dayLabel(new Date(year, month, day, 12), clock.now);
}

/**
 * The days from `from` to `to`, as a line says them: one day as `dayLabel` does; a range with no
 * weekdays, "Oct 3 – 5", "Sep 29 – Oct 2", "Dec 30, 2025 – Jan 2", "Oct 3 – 5, 2025", ending
 * "– Today" or "– Yesterday" — "Oct 9 – Today", "Yesterday – Today", "Dec 30, 2025 – Yesterday".
 */
function dayRangeWords(from: number, to: number, clock: DayClock): string {
  if (from >= to) return dayWords(to, clock);
  const named = (key: number): string | null => (key === clock.today ? "Today" : key === clock.yesterday ? "Yesterday" : null);
  const start = dayParts(from);
  const end = dayParts(to);
  const date = (parts: { year: number; month: number; day: number }, withYear: boolean): string =>
    `${DAY_MONTHS[parts.month]} ${parts.day}${withYear ? `, ${parts.year}` : ""}`;
  if (named(to)) return `${named(from) || date(start, start.year !== clock.year)} – ${named(to)}`;
  const tail = start.year === end.year && start.year !== clock.year ? `, ${start.year}` : "";
  if (start.year === end.year && start.month === end.month) return `${date(start, false)} – ${end.day}${tail}`;
  if (start.year === end.year) return `${date(start, false)} – ${date(end, false)}${tail}`;
  return `${date(start, start.year !== clock.year)} – ${date(end, end.year !== clock.year)}`;
}

/** A day inside a sentence: "today", "yesterday", "Oct 3", or "Oct 3, 2025" for another year's. */
function searchDayWords(key: number, clock: DayClock = dayClock()): string {
  if (key === clock.today) return "today";
  if (key === clock.yesterday) return "yesterday";
  const { year, month, day } = dayParts(key);
  return `${DAY_MONTHS[month]} ${day}${year !== clock.year ? `, ${year}` : ""}`;
}

export { DAY_MONTHS, DAY_WEEKDAYS, dayClock, dayKeyOf, dayLabel, dayParts, dayRangeWords, dayTitle, dayWords, sameLocalDay, searchDayWords };
export type { DayClock };

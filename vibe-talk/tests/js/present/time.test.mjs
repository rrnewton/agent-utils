// Tests for web/src/present/time/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the same
// module, in named zones and on the page's own clock, is tested in ../page/rows.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** How the page says when: days, ranges of days, ages, a message's time (web/src/present/time). */
const TIME = await moduleForTests("src/present/time/index.ts");

/** A local instant: month counted from 1, as people count it; noon unless said. */
const localAt = (year, month, day, hour = 12, minute = 0) => new Date(year, month - 1, day, hour, minute);

test("`#624 voice-page-modularity`: a day, a run of days and a day in a sentence, from the module alone", () => {
  const { dayLabel, dayRangeWords, searchDayWords, dayKeyOf } = TIME;
  const now = localAt(2026, 10, 12, 15, 56);
  assert.equal(dayLabel(localAt(2026, 10, 12, 9), now), "Today, Oct 12, Mon");
  assert.equal(dayLabel(localAt(2026, 10, 11, 17, 25), now), "Yesterday, Oct 11, Sun");
  assert.equal(dayLabel(localAt(2025, 10, 9), now), "Oct 9, 2025, Thu");
  // A pass's clock, as `dayClock` reads it, but at a chosen instant.
  const clock = { now, today: dayKeyOf(now), yesterday: dayKeyOf(localAt(2026, 10, 11)), year: 2026 };
  assert.equal(dayRangeWords(20261007, 20261009, clock), "Oct 7 – 9");
  assert.equal(dayRangeWords(20251230, 20260102, clock), "Dec 30, 2025 – Jan 2");
  assert.equal(dayRangeWords(20261011, 20261012, clock), "Yesterday – Today");
  assert.equal(dayRangeWords(20261012, 20261012, clock), "Today, Oct 12, Mon");
  assert.deepStrictEqual([20261012, 20261011, 20261003, 20251003].map((key) => searchDayWords(key, clock)),
    ["today", "yesterday", "Oct 3", "Oct 3, 2025"]);
});

test("`#624 voice-page-modularity`: an age on its one scale, truncated, and a message's time, from the module alone", () => {
  const { briefAge, messageDate, shortLocalTime, fullLocalTime } = TIME;
  const ago = (ms) => Date.now() - ms;
  assert.deepStrictEqual([ago(30e3), ago(59 * 60e3), ago(23 * 3600e3), ago(42 * 3600e3), ago(72 * 3600e3),
    ago(12 * 86400e3), ago(42 * 86400e3), ago(800 * 86400e3)].map(briefAge),
  ["now", "59m", "23h", "1d18h", "3d", "12d", "6w", "2y"]);
  assert.equal(briefAge(Number.NaN), "");
  assert.equal(messageDate({ timestamp: "not a time" }), null);
  assert.equal(shortLocalTime({ timestamp: "", spoken_time: "five past nine" }), "five past nine");
  assert.equal(fullLocalTime({}), "unknown");
  const at = localAt(2026, 8, 19, 9, 10);
  assert.equal(shortLocalTime({ timestamp: at.toISOString() }), at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }));
});

test("`#729 vt-older-message-dates`: a row's time is the time today, the weekday and time up to six days back, the date and time before that", () => {
  const { messageTimeLabel, calendarDaysBefore, shortTimeFormat } = TIME;
  const was = process.env.TZ;
  process.env.TZ = "America/New_York";
  try {
    const time = (at) => shortTimeFormat().format(at);
    const label = (at, now) => messageTimeLabel({ timestamp: at.toISOString() }, now);
    // Sunday Nov 1 2026, 15:56 in New York: the clocks went back at 02:00 that morning.
    const now = localAt(2026, 11, 1, 15, 56);
    const cases = [
      [localAt(2026, 11, 1, 0, 1), `${time(localAt(2026, 11, 1, 0, 1))}`, "today, a minute past midnight"],
      [localAt(2026, 10, 31, 23, 59), `Sat ${time(localAt(2026, 10, 31, 23, 59))}`, "yesterday, a minute before midnight"],
      [localAt(2026, 10, 26, 15, 23), `Mon ${time(localAt(2026, 10, 26, 15, 23))}`, "six days back"],
      [localAt(2026, 10, 25, 15, 23), `10/25 ${time(localAt(2026, 10, 25, 15, 23))}`, "seven days back"],
      [localAt(2025, 11, 1, 9, 0), `11/1 ${time(localAt(2025, 11, 1, 9, 0))}`, "a year back, the same day"],
    ];
    for (const [at, want, why] of cases) assert.equal(label(at, now), want, why);
    // The day the clocks went back is 25 hours long: 24 hours before 00:30 Monday is still Sunday.
    const monday = localAt(2026, 11, 2, 0, 30);
    assert.equal(calendarDaysBefore(localAt(2026, 11, 1, 1, 0), monday), 1);
    assert.equal(label(localAt(2026, 11, 1, 1, 0), monday), `Sun ${time(localAt(2026, 11, 1, 1, 0))}`);
    // Spring forward: Sunday Mar 8 is 23 hours long, and Saturday night is two days before Monday.
    assert.equal(calendarDaysBefore(localAt(2026, 3, 7, 23, 45), localAt(2026, 3, 9, 0, 30)), 2);
    // A clock behind the message's: not today, so its date.
    assert.equal(label(localAt(2026, 11, 2, 9, 0), now), `11/2 ${time(localAt(2026, 11, 2, 9, 0))}`);
    // No instant: the server's words, as ever.
    assert.equal(messageTimeLabel({ timestamp: "", spoken_time: "3:23 PM" }, now), "3:23 PM");
  } finally {
    if (was === undefined) delete process.env.TZ;
    else process.env.TZ = was;
  }
});

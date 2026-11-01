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

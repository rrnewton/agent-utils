"use strict";

// Deep-link hashes are the shareable form of a timeline place, so the
// contract that matters is the round trip: a place the page serializes must
// parse back to the same place, and a hand-written `#view=day&t=...` must
// land on that calendar day in the archive timezone, DST included. The
// parser and serializer are pure functions in app.js; this suite extracts
// them the same way the zoom suite does, without a DOM. History behaviour
// (push versus replace, Back) needs a real browser and is covered by the
// Playwright suite.

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const source = fs.readFileSync(
  path.resolve(__dirname, "../../wrkviz/static/app.js"),
  "utf8"
);

function functionSource(name) {
  const marker = "  function " + name + "(";
  const start = source.indexOf(marker);
  assert.notStrictEqual(start, -1, "missing function " + name);
  const next = source.indexOf("\n  function ", start + marker.length);
  assert.notStrictEqual(next, -1, "missing function boundary after " + name);
  return source.slice(start, next);
}

const context = {
  Date: Date,
  Intl: Intl,
  Math: Math,
  Number: Number,
  Object: Object
};
vm.createContext(context);
vm.runInContext(
  [
    functionSource("isGlossaryHash"),
    functionSource("deepLinkViewForSpan"),
    functionSource("parseDeepLinkDate"),
    functionSource("deepLinkWallParts"),
    functionSource("deepLinkDateText"),
    functionSource("deepLinkZonedMidnightMs"),
    functionSource("deepLinkCalendarWindow"),
    functionSource("parseDeepLinkHash"),
    functionSource("serializeDeepLinkState")
  ].join("\n"),
  context
);

// Objects built inside the vm realm carry that realm's prototype, which
// deepStrictEqual would reject even when every field matches.
function plain(value) {
  return JSON.parse(JSON.stringify(value));
}

const dayStart = Date.UTC(2026, 10, 8);
const dayEnd = Date.UTC(2026, 10, 9);
const canonical =
  "#view=day&t=2026-11-08&start=" + dayStart + "&end=" + dayEnd +
  "&team=team-alpha&project=example-project";

assert.strictEqual(
  context.serializeDeepLinkState({
    start_ms: dayStart,
    end_ms: dayEnd,
    team: "team-alpha",
    project: "example-project"
  }, "UTC"),
  canonical,
  "a UTC day window serializes in canonical order"
);
assert.deepStrictEqual(plain(context.parseDeepLinkHash(canonical)), {
  start_ms: dayStart,
  end_ms: dayEnd,
  team: "team-alpha",
  view: "day",
  t: "2026-11-08",
  project: "example-project"
}, "the canonical hash parses back to the same place");

const reordered =
  "#project=example-project&end=" + dayEnd + "&team=team-alpha" +
  "&t=2026-11-08&start=" + dayStart + "&view=daily&unrelated=ignored";
assert.deepStrictEqual(
  plain(context.parseDeepLinkHash(reordered)),
  plain(context.parseDeepLinkHash(canonical)),
  "parameter order, rollup-kind view aliases, and unknown keys do not change the place"
);

const spacedProject = context.serializeDeepLinkState({
  start_ms: dayStart,
  end_ms: dayStart + 60 * 1000,
  team: "",
  project: "example project"
}, "UTC");
assert.strictEqual(
  context.parseDeepLinkHash(spacedProject).project,
  "example project",
  "project labels that need escaping survive the round trip"
);
assert.strictEqual(
  context.parseDeepLinkHash(spacedProject).team,
  "",
  "an absent team means all teams"
);

// The glossary owns its hashes: timeline state must neither claim nor
// produce them, or a pasted glossary link would stop opening its entry.
["#glossary", "#glossary/term-example", "", "#", "#unrelated", "#unrelated=1"]
  .forEach(function (hash) {
    assert.strictEqual(
      context.parseDeepLinkHash(hash),
      null,
      "not a timeline place: " + JSON.stringify(hash)
    );
  });

// A bad window discards only the window, so a link with one bad number can
// still be read as its calendar or team shorthand instead of zooming to a
// nonsense range.
[
  "#start=soon&end=" + dayEnd + "&team=team-alpha",
  "#start=" + dayStart + ".5&end=" + dayEnd + "&team=team-alpha",
  "#start=" + dayEnd + "&end=" + dayStart + "&team=team-alpha",
  "#start=" + dayStart + "&team=team-alpha"
].forEach(function (hash) {
  const parsed = context.parseDeepLinkHash(hash);
  assert.strictEqual(parsed.start_ms, null, hash);
  assert.strictEqual(parsed.end_ms, null, hash);
  assert.strictEqual(parsed.team, "team-alpha", hash);
});
assert.deepStrictEqual(plain(context.parseDeepLinkHash("#view=yearly&t=2026-02-30")), {
  start_ms: null,
  end_ms: null,
  team: "",
  view: "",
  t: "",
  project: ""
}, "an unknown view and an impossible date are dropped, not guessed at");
assert.strictEqual(
  context.serializeDeepLinkState({ start_ms: dayEnd, end_ms: dayStart }, "UTC"),
  "",
  "a non-positive window has no hash to write"
);

// When both forms are present the parser keeps both; application prefers
// the exact milliseconds. The view stays in the hash for the human who was
// pasted the link, not as a second machine input.
const exactAndCalendar = context.parseDeepLinkHash(
  "#view=month&t=2026-01-15&start=" + dayStart + "&end=" + dayEnd
);
assert.strictEqual(exactAndCalendar.start_ms, dayStart);
assert.strictEqual(exactAndCalendar.view, "month");
assert.strictEqual(exactAndCalendar.t, "2026-01-15");

const hour = 60 * 60 * 1000;
assert.strictEqual(context.deepLinkViewForSpan(0, 25 * hour), "day");
assert.strictEqual(context.deepLinkViewForSpan(0, 7 * 24 * hour), "week");
assert.strictEqual(context.deepLinkViewForSpan(0, 31 * 24 * hour), "month");
assert.strictEqual(context.deepLinkViewForSpan(0, 92 * 24 * hour), "quarter");
assert.strictEqual(context.deepLinkViewForSpan(0, 200 * 24 * hour), "range");

assert.deepStrictEqual(
  plain(context.deepLinkCalendarWindow("day", "2026-11-08", "UTC")),
  { start_ms: dayStart, end_ms: dayEnd }
);
assert.deepStrictEqual(
  plain(context.deepLinkCalendarWindow("week", "2026-11-08", "UTC")),
  { start_ms: Date.UTC(2026, 10, 2), end_ms: Date.UTC(2026, 10, 9) },
  "a week is the Monday-start week containing the date, matching weekly rollups"
);
assert.deepStrictEqual(
  plain(context.deepLinkCalendarWindow("month", "2026-11-08", "UTC")),
  { start_ms: Date.UTC(2026, 10, 1), end_ms: Date.UTC(2026, 11, 1) }
);
assert.deepStrictEqual(
  plain(context.deepLinkCalendarWindow("quarter", "2026-11-08", "UTC")),
  { start_ms: Date.UTC(2026, 9, 1), end_ms: Date.UTC(2027, 0, 1) }
);
assert.strictEqual(context.deepLinkCalendarWindow("range", "2026-11-08", "UTC"), null);

// Archive days are wall-clock days in the display timezone, so the two DST
// days are 23 and 25 hours long. A fixed 24-hour window would drift an
// hour off the day a reader named.
const zone = "America/New_York";
const springDay = context.deepLinkCalendarWindow("day", "2026-03-08", zone);
assert.strictEqual(springDay.start_ms, Date.UTC(2026, 2, 8, 5));
assert.strictEqual(springDay.end_ms - springDay.start_ms, 23 * hour);
const fallDay = context.deepLinkCalendarWindow("day", "2026-11-01", zone);
assert.strictEqual(fallDay.start_ms, Date.UTC(2026, 10, 1, 4));
assert.strictEqual(fallDay.end_ms - fallDay.start_ms, 25 * hour);
assert.strictEqual(
  context.serializeDeepLinkState(
    { start_ms: springDay.start_ms, end_ms: springDay.end_ms, team: "", project: "" },
    zone
  ),
  "#view=day&t=2026-03-08&start=" + springDay.start_ms + "&end=" + springDay.end_ms,
  "serializing a zoned calendar window names the same zoned day back"
);

console.log("timeline deep link tests passed");

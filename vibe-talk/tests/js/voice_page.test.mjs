// Tests for the app (the page at /), run against the REAL web/voice.js, web/voice.html and
// web/voice.css: the entry every run of the suite starts from (`#624 voice-page-modularity`).
//
// The fixture, and what it can and cannot show, is tests/js/page/fixture.mjs. The tests are in
// tests/js/page/<feature>.mjs, a file per feature, imported below in a fixed order: a test's position
// is what `VOICE_PAGE_SHARD` slices by, so it must not hang on the order a directory lists. A
// feature's file also runs alone: node --test tests/js/page/<feature>.mjs.
//
// Every file in tests/js/page/ but the fixture is imported here, or this file throws as it loads: a
// feature's file that nothing imports would never run.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { testsDefined } from "./page/fixture.mjs";
import "./page/ground-rules.mjs";
import "./page/shell.mjs";
import "./page/settings.mjs";
import "./page/composer.mjs";
import "./page/call.mjs";
import "./page/call-sending.mjs";
import "./page/read-aloud.mjs";
import "./page/rows.mjs";
import "./page/links.mjs";
import "./page/scrolling.mjs";
import "./page/keyboard.mjs";
import "./page/replies.mjs";
import "./page/threads.mjs";
import "./page/columns.mjs";
import "./page/done.mjs";
import "./page/navigation.mjs";
import "./page/reading-state.mjs";
import "./page/search.mjs";
import "./page/sync.mjs";
import "./page/sending.mjs";

// Raise this with the suite. It is a floor against a suite that stopped COLLECTING, so it has to
// track the real count; left behind, it would keep passing while half the suite went missing.
const COLLECTED_FLOOR = 294;
process.on("exit", () => {
  if (testsDefined < COLLECTED_FLOOR) {
    console.error(`voice_page: the suite defines only ${testsDefined} tests, so something stopped collecting them`);
    process.exitCode = 1;
  }
});

const SELF = fileURLToPath(import.meta.url);
const imported = [...readFileSync(SELF, "utf8").matchAll(/^import "\.\/page\/([\w-]+\.mjs)";$/gm)].map((m) => m[1]);
const unimported = readdirSync(join(dirname(SELF), "page"))
  .filter((name) => name.endsWith(".mjs") && name !== "fixture.mjs" && !imported.includes(name));
if (unimported.length > 0) {
  throw new Error(`tests/js/page/ holds feature files this suite does not import: ${unimported.join(", ")}`);
}

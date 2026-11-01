// Tests for web/src/present/notices/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the same
// module, where it says these things, is tested in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** What the page says in its notices and status lines (web/src/present/notices). */
const NOTICES = await moduleForTests("src/present/notices/index.ts");

test("`#624 voice-page-modularity`: counts, sizes and durations in the page's fixed English, from the module alone", () => {
  const { nounCount, cacheSize, fmtDuration, channelHistoryDuration } = NOTICES;
  assert.deepStrictEqual([nounCount(1, "day"), nounCount(1200, "message"), nounCount(0, "channel")],
    ["1 day", "1,200 messages", "0 channels"]);
  assert.deepStrictEqual([cacheSize(0), cacheSize(10), cacheSize(3 * 1024 * 1024)], ["0 KB", "1 KB", "3.0 MB"]);
  assert.deepStrictEqual([fmtDuration(120), fmtDuration(1900), fmtDuration(null), fmtDuration("x")], ["120 ms", "1.9 s", "—", "—"]);
  assert.deepStrictEqual([60, 600, 3 * 3600, 3 * 86400].map(channelHistoryDuration),
    ["a minute", "10 minutes", "about 3 hours", "about 3 days"]);
});

test("`#624 voice-page-modularity`: failures and the server's states in the reader's words, from the module alone", () => {
  const { refusalSentence, refusalWords, failureWords, serviceEndedSentence, serverCacheLine, serverCacheErases } = NOTICES;
  assert.match(refusalSentence({ status: 403 }), /READ token/);
  assert.match(refusalSentence({ status: 401 }), /was refused/);
  assert.equal(refusalWords({ explanation: "it names @Ada." }, "google chat"), "Google chat refused this message: it names @Ada.");
  assert.equal(refusalWords(null, "slack"), "");
  assert.equal(failureWords({ detail: "the read timed out." }), "the read timed out");
  assert.equal(failureWords(""), "no reason was given");
  assert.match(serviceEndedSentence("3 minutes", true, ""), /^The call dropped 3 minutes in:/);
  assert.match(serviceEndedSentence("", false, "bye."), /ended the call — you did not hang up\. It said: bye\./);
  const off = { enabled: false, available: false, reason: "switched_off", messages: 0, channels: 0, bytes: 0, max_days: 7, max_megabytes: 64 };
  assert.match(serverCacheLine(off), /^Off: you turned it off/);
  assert.equal(serverCacheLine({ ...off, enabled: true, reason: "locked" }), "On, but no file is open, so nothing is being saved: another process holds the file.");
  assert.equal(serverCacheErases({ ...off, available: true, messages: 2, bytes: 2048 }), "the 2 saved messages (2 KB)");
});

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

test("`#624 voice-page-modularity`: a search's reach and a message marked read, in the reader's words, from the module alone", () => {
  const { messagesWords, reachClause, reachBackWords, searchWalkProgress, serviceReadWords, doneReadWords } = NOTICES;
  assert.deepStrictEqual([messagesWords(1), messagesWords(82)], ["1 message", "82 messages"]);
  assert.equal(reachBackWords(null), "");
  const reach = { total: 9, proven: true, saved: false, dense: 5, scattered: 4, from: null, start: false, whole: false,
    topLevel: false, threads: 0, full: 0 };
  assert.equal(reachClause(reach), "a complete history back to the oldest loaded (no gaps), plus 4 scattered older ones");
  assert.equal(reachClause({ ...reach, topLevel: true, start: true, scattered: 0, threads: 31, full: 20 }),
    "every top-level message back to the first one, and every reply in 20 of their 31 threads");
  assert.deepStrictEqual([searchWalkProgress({ arrived: 0 }), searchWalkProgress({ arrived: 52 })],
    ["Fetching older messages… · Stop", "Fetching… 52 older messages · Stop"]);
  assert.equal(serviceReadWords({ service: "Slack", marked: false, why: "no scope" }, ", and collapsed"),
    "Marked read here, not in Slack, and collapsed: no scope");
  assert.equal(doneReadWords({ service: "Slack", marked: true, already: true, why: "" }), "Already read here and in Slack.");
});

test("`#624 voice-page-modularity`: what a walk further back says when it ends, from the module alone", () => {
  const { searchWalkWords } = NOTICES;
  let asked = [];
  const facts = (oldest = null) => ({ service: "Slack", budgetSeconds: 20, oldest,
    viewWords: () => { asked.push("view"); return "this channel"; },
    failure: () => { asked.push("failure"); return "the network is down"; } });
  const walk = (arrived, found, read = 1) => ({ arrived, found, read });
  assert.equal(searchWalkWords(walk(0, 0), "failed", facts()),
    "Could not fetch older messages from Slack: the network is down. Nothing older was searched. Press the button again to retry.");
  assert.deepStrictEqual(asked, ["failure"], "the view's words were asked for a failure");
  asked = [];
  assert.equal(searchWalkWords(walk(0, 0), "nowhere", facts()), "Nothing older can be fetched: Slack gave no way further back.");
  assert.equal(searchWalkWords(walk(0, 0, 0), "stopped", facts()), "Stopped before any older messages arrived.");
  assert.equal(searchWalkWords(walk(0, 0), "start", facts()), "Reached the start of this channel.");
  assert.deepStrictEqual(asked, ["view"]);
  assert.equal(searchWalkWords(walk(3, 1), "start", facts()), "Reached the start of this channel. Found 1 more match among 3 older messages fetched from Slack.");
  assert.equal(searchWalkWords(walk(0, 0), "budget", facts()),
    "Slack sent nothing older this time, though it says older messages exist. Press the button again to keep going.");
  assert.equal(searchWalkWords(walk(50, 2), "budget", facts()), "Found 2 more matches among 50 older messages fetched from Slack in 20 s.");
  assert.equal(searchWalkWords(walk(1, 0), "stopped", facts()), "Stopped. None of the 1 older message fetched from Slack match.");
});

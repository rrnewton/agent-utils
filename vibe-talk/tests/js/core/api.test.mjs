// Tests for web/src/core/api/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The page's requests to its server, on ports a test supplies (web/src/core/api). */
const SERVER_API = await moduleForTests("src/core/api/index.ts");

test("`#624 voice-page-modularity`: a failed request in words, from the requests module alone", () => {
  const { readableFailure, isSlowFailure, providerWords } = SERVER_API;
  assert.equal(providerWords('Slack returned HTTP 502: {"message":"upstream timed out"}'), "upstream timed out");
  assert.equal(providerWords('X returned HTTP 502: {"message":"cut sho'), "X returned HTTP 502");
  assert.equal(providerWords('X: {"detail":"half \\"quoted\\" words","more'), 'half "quoted" words');
  assert.equal(isSlowFailure({ status: 502, detail: "deadline exceeded" }), true);
  assert.equal(isSlowFailure({ status: 400, detail: "deadline exceeded" }), false);
  assert.equal(readableFailure({ status: 504 }, "discord"), "Discord is slow to answer right now.");
  assert.equal(readableFailure({ status: 502, detail: "" }, "slack"), "Slack could not answer (HTTP 502).");
  assert.equal(readableFailure({ status: 409, message: "HTTP 409 moved: x" }, "slack"), "HTTP 409 moved: x",
    "a refusal below 500 is the server's own sentence");
  assert.equal(readableFailure(null, "slack"), "");
});

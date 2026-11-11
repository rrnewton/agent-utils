// Tests for web/src/view/shell/deployment.ts on its own (`#624 voice-page-modularity`):
// which deployment's origin and host are shown, and redacting the browser's token (and
// the token as typed) out of anything the page might print. The page over the same
// factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

function makeWords(location, kept = {}) {
  globalThis.window = { location };
  return SHELL.createDeploymentWords({
    token: () => kept.token || "",
    tokenField: () => ({ value: kept.field || "" }),
  });
}

test("the deployment origin and host are trimmed, with the page's fallbacks", () => {
  const words = makeWords({ origin: " https://chat.example:8443 ", host: " chat.example:8443 " });
  assert.equal(words.deploymentOrigin(), "https://chat.example:8443");
  assert.equal(words.deploymentHost(), "chat.example:8443");

  const hostless = makeWords({ origin: "https://chat.example/base", host: "" });
  assert.equal(hostless.deploymentHost(), "chat.example", "no host falls back to the origin's authority");

  const empty = makeWords({ origin: "", host: "" });
  assert.equal(empty.deploymentOrigin(), "this server");
  assert.equal(empty.deploymentHost(), "this server");

  const missing = makeWords({});
  assert.equal(missing.deploymentOrigin(), "this server");
});

test("redact replaces every token of credential length, and nothing shorter", () => {
  const words = makeWords(
    { origin: "https://chat.example", host: "chat.example" },
    { token: "secret-token-123", field: "typed-secret-456" },
  );
  assert.equal(words.redact("failed with secret-token-123 in it"), "failed with [redacted] in it");
  assert.equal(words.redact("secret-token-123 and typed-secret-456"), "[redacted] and [redacted]");
  assert.equal(words.redact("no secrets here"), "no secrets here");
  assert.equal(words.redact(42), "42");

  const short = makeWords(
    { origin: "https://chat.example", host: "chat.example" },
    { token: "see", field: "a" },
  );
  assert.equal(short.redact("see a thing"), "see a thing", "short strings are not credentials and are never blanked out");
});

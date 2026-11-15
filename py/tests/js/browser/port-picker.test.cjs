"use strict";

// #1687 timeline-browser-fixed-port: a busy derived port must not break the
// browser checks. The picker is tested with injected probes for the decision
// table, and once against real sockets for the busy-port case itself.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const net = require("net");
const { derivedPort, pickPort, probe } = require("./port-picker.cjs");

test("the derived port is stable and in the derived range", () => {
  const port = derivedPort(__dirname);
  assert.equal(port, derivedPort(__dirname));
  assert.ok(port >= 21000 && port < 39000, "derived port out of range: " + port);
});

test("a pinned port wins and is never probed", () => {
  let probed = false;
  const port = pickPort({
    pinned: "45678",
    preferred: 21001,
    probeFn: () => {
      probed = true;
      return null;
    },
  });
  assert.equal(port, 45678);
  assert.equal(probed, false);
});

test("a bad pin is refused, not silently moved", () => {
  assert.throws(() => pickPort({ pinned: "not-a-port", preferred: 21001 }), /not a port/);
});

test("a free derived port is used as derived", () => {
  const port = pickPort({ preferred: 23456, probeFn: (want) => want });
  assert.equal(port, 23456);
});

test("a busy derived port falls back to an ephemeral one", () => {
  const port = pickPort({
    preferred: 23456,
    probeFn: (want) => (want === 0 ? 39123 : null),
  });
  assert.equal(port, 39123);
});

test("no free port at all is an error naming the derived port", () => {
  assert.throws(
    () => pickPort({ preferred: 23456, probeFn: () => null }),
    /derived port 23456 is busy/
  );
});

test("a port a real socket holds cannot be picked", async () => {
  const holder = net.createServer();
  await new Promise((resolve) => holder.listen(0, "127.0.0.1", resolve));
  const busy = holder.address().port;
  try {
    assert.equal(probe(busy), null, "the probe called a held port free");
    const port = pickPort({ preferred: busy });
    assert.notEqual(port, busy);
    assert.equal(probe(port), port, "the picked port is not actually free");
  } finally {
    await new Promise((resolve) => holder.close(resolve));
  }
});

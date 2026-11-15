"use strict";

// The fixture server's port, chosen at run time (#1687 timeline-browser-fixed-port).
//
// The port is derived from this checkout's path, so parallel gates in
// different checkouts never collide on one fixed port. But a derived port can
// still be held by an unrelated program on the host, and then the webServer
// cannot bind and every browser check fails before a single test runs. So the
// derived port is only a preference: when something already listens on it, an
// ephemeral port the kernel picks is used instead. Pin
// AGENT_TIMELINE_E2E_PORT to choose a specific port (a pin is honoured as
// given; a busy pin fails loudly rather than silently moving).

const crypto = require("crypto");
const { execFileSync } = require("child_process");

function derivedPort(dirname) {
  const digest = crypto.createHash("sha256").update(dirname).digest();
  return 21000 + (digest.readUInt16BE(0) % 18000);
}

// Bind `want` (0 for an ephemeral port) on 127.0.0.1 in a child process and
// report the bound port, or null when the bind fails. A child process keeps
// the probe synchronous for config loading and leaves nothing listening.
const PROBE =
  'const s=require("net").createServer();' +
  's.once("error",()=>process.exit(1));' +
  's.listen(Number(process.argv[1]),"127.0.0.1",()=>{console.log(s.address().port);s.close();});';

function probe(want) {
  try {
    const out = execFileSync(process.execPath, ["-e", PROBE, String(want)], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    const port = Number(out.trim());
    return Number.isInteger(port) && port > 0 ? port : null;
  } catch {
    return null;
  }
}

// The port to serve on: the pinned one when pinned, else the derived one when
// free, else an ephemeral one. `probeFn` is injectable for tests.
function pickPort({ pinned, preferred, probeFn = probe }) {
  if (pinned !== undefined && pinned !== null && pinned !== "") {
    const port = Number(pinned);
    if (!Number.isInteger(port) || port <= 0 || port > 65535) {
      throw new Error("AGENT_TIMELINE_E2E_PORT is not a port: " + pinned);
    }
    return port;
  }
  if (probeFn(preferred) === preferred) return preferred;
  const free = probeFn(0);
  if (free === null) {
    throw new Error(
      "no free port for the timeline fixture server (derived port " +
        preferred +
        " is busy and no ephemeral port could be bound)"
    );
  }
  return free;
}

module.exports = { derivedPort, pickPort, probe };

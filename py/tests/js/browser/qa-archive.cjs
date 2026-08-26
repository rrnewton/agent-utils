#!/usr/bin/env node
"use strict";

// Benchmark a REAL generated archive at four zoom levels, using its own `serve.py`.
//
// The suite beside this runs against a four-agent fixture. That fixture is the right thing to
// gate on -- it is deterministic, it is committed, and it runs in twenty seconds -- but it cannot
// answer the question this file exists for: does the tool stay responsive on an archive with
// thousands of agents and hundreds of thousands of tool calls. Nothing about a four-agent page
// exercises the level-of-detail machinery, because at that size every level renders everything.
//
// So this is manual QA, deliberately not a gate: it needs an archive the repository does not
// contain and must never contain. Point it at one you have built.
//
//     node qa-archive.cjs --archive ~/logs/summary/myproject
//
// WHY THREE LEVELS. `timeline-core.semanticZoomLevel` picks a level of detail from
// milliseconds-per-pixel -- `detail` at or under a minute per pixel, `lifetime` at or under five,
// `aggregate` beyond -- so one archive draws three substantially different scenes depending only
// on how far out the reader is. Measuring the fitted view alone measures the cheapest of them and
// reports it as the tool's performance. The three levels here are chosen to land in the three
// regimes on a normal window: a fitted multi-week archive is `aggregate`, a day is `detail`, and
// a week is usually the boundary.
//
// The archive's OWN `serve.py` is what serves it, not a fixture server, because byte-range
// answers over the schema-3 shards are part of what is being measured and that is the server
// that ships inside the archive.

const childProcess = require("node:child_process");
const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");

//: Four, not three. `day` lands in `lifetime` rather than `detail` on a normal window -- a day
//: across ~1400px is about 61s per pixel against a 60s threshold -- so `hour` is what actually
//: exercises the level that draws individual phases, which is the expensive one.
//:
//: A KNOWN, UNRESOLVED FAILURE, recorded because a retry hides it otherwise. The fourth level of
//: a run whose levels DIFFER fails to load, whichever level is fourth: `hour` when it is fourth,
//: `day` when it is fourth. Established by experiment: every level passes alone; any two pass
//: together; `fit,fit,fit,fit` passes; three differing levels pass. It presents as a 120s load
//: timeout with no page error, no console error and no failed request, against a server started
//: fresh for that level. Ruled out: memory (181G of 1010G used), /dev/shm (504G free), server
//: teardown (process-group kill plus a settle delay did not help). The remaining suspect is state
//: accumulating in this parent process across sequential Playwright launches. Each level is
//: retried once, which makes the tool usable and is not a fix.
const LEVELS = ["fit", "week", "day", "hour"];

function parseArguments(argv) {
  const options = { archive: "", jsonPath: "", timeoutMs: 120_000, levels: LEVELS.slice() };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "-h" || argument === "--help") {
      options.help = true;
    } else if (argument === "--levels") {
      const value = argv[index + 1];
      if (!value) {
        throw new Error("--levels requires a comma-separated list");
      }
      index += 1;
      options.levels = value.split(",").map(function (item) { return item.trim(); });
      const unknown = options.levels.filter(function (item) { return LEVELS.indexOf(item) < 0; });
      if (unknown.length) {
        throw new Error("unknown level(s): " + unknown.join(", "));
      }
    } else if (argument === "--archive" || argument === "--json" || argument === "--timeout-ms") {
      const value = argv[index + 1];
      if (!value) {
        throw new Error(argument + " requires a value");
      }
      index += 1;
      if (argument === "--archive") {
        options.archive = value;
      } else if (argument === "--json") {
        options.jsonPath = value;
      } else {
        options.timeoutMs = Number(value);
      }
    } else if (!options.archive) {
      options.archive = argument;
    } else {
      throw new Error("unexpected argument: " + argument);
    }
  }
  return options;
}

async function unusedPort() {
  return new Promise(function (resolve, reject) {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", function () {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      server.close(function (error) {
        if (error) {
          reject(error);
        } else {
          resolve(port);
        }
      });
    });
  });
}

async function stopGroup(child) {
  // Signal the GROUP (negative pid), then wait, then SIGKILL the group. Signalling the pid alone
  // leaves anything the server spawned, and leaves the server itself if it declines SIGTERM.
  const done = new Promise(function (resolve) { child.once("exit", resolve); });
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch (_error) {
    // Already gone.
  }
  const exited = await Promise.race([
    done.then(function () { return true; }),
    new Promise(function (resolve) { setTimeout(function () { resolve(false); }, 5_000); })
  ]);
  if (!exited) {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch (_error) {
      // Already gone.
    }
    await Promise.race([
      done,
      new Promise(function (resolve) { setTimeout(resolve, 3_000); })
    ]);
  }
}


async function waitForServer(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const reachable = await new Promise(function (resolve) {
      const socket = net.connect(port, "127.0.0.1");
      socket.once("connect", function () {
        socket.destroy();
        resolve(true);
      });
      socket.once("error", function () {
        resolve(false);
      });
    });
    if (reachable) {
      return;
    }
    await new Promise(function (resolve) { setTimeout(resolve, 200); });
  }
  throw new Error("the archive's serve.py did not start within " + timeoutMs + "ms");
}

function runBenchmark(url, level, timeoutMs) {
  const result = childProcess.spawnSync(
    process.execPath,
    [
      path.join(__dirname, "benchmark-site.cjs"),
      "--url", url,
      "--zoom", level,
      "--timeout-ms", String(timeoutMs)
    ],
    { cwd: __dirname, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 }
  );
  if (result.error) {
    throw new Error("benchmark could not run at zoom=" + level + ": " + result.error.message);
  }
  if (result.status !== 0 || result.signal) {
    // Status AND signal AND both streams. Reporting only stderr hid a killed child behind an
    // empty message: a process terminated by a signal has status null and says nothing, which
    // reads as "failed for no reason".
    throw new Error(
      "benchmark failed at zoom=" + level +
      " (status=" + String(result.status) + " signal=" + String(result.signal) + ")" +
      "\n  stderr: " + (result.stderr || "<empty>").slice(-1500) +
      "\n  stdout: " + (result.stdout || "<empty>").slice(-500)
    );
  }
  return JSON.parse(result.stdout);
}

function formatSpan(ms) {
  if (!Number.isFinite(ms)) {
    return "?";
  }
  const hours = ms / 3_600_000;
  if (hours >= 48) {
    return (hours / 24).toFixed(1) + "d";
  }
  return hours.toFixed(1) + "h";
}

function renderTable(reports) {
  const header =
    "  " + "level".padEnd(7) + "span".padStart(8) + "  " + "lod".padEnd(10) +
    "tracks".padEnd(11) + "svg".padStart(7) + "edges".padStart(7) + "bins".padStart(7) +
    "draw".padStart(9) + "p50".padStart(9) + "p95".padStart(9) + "max".padStart(9);
  const rows = reports.map(function (entry) {
    const report = entry.report;
    const shape = report.timeline.initial;
    const input = report.interaction.input_to_raf_ms;
    return "  " +
      entry.level.padEnd(7) +
      formatSpan(report.zoom.reached_span_ms).padStart(8) + "  " +
      String(shape.lod).padEnd(10) +
      String(shape.track_mode).padEnd(11) +
      String(shape.counts.svg_nodes).padStart(7) +
      String(shape.counts.edges).padStart(7) +
      String(shape.counts.aggregate_bins).padStart(7) +
      (String(shape.render_duration_ms) + "ms").padStart(9) +
      (String(input.p50) + "ms").padStart(9) +
      (String(input.p95) + "ms").padStart(9) +
      (String(input.max) + "ms").padStart(9) +
      (entry.retried ? "  (retried)" : "");
  });
  return [
    "",
    "  archive redraw benchmark -- zoom/pan latency by level of detail",
    "  draw = the page's own last render; p50/p95/max = wheel event to the frame reflecting it",
    "",
    header,
    "  " + "-".repeat(header.length - 2),
    ...rows,
    ""
  ].join("\n");
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  if (options.help || !options.archive) {
    process.stdout.write(
      "Usage: node qa-archive.cjs --archive <built archive directory> [options]\n\n" +
      "  --levels a,b,c   subset of: " + LEVELS.join(", ") + " (default: all)\n" +
      "  --json <path>    also write the full per-level reports as JSON\n\n" +
      "Benchmarks zoom/pan redraw by level of detail against a real archive, served by\n" +
      "the archive's own serve.py. Manual QA: not a gate, and it needs an archive this\n" +
      "repository does not ship.\n"
    );
    process.exitCode = options.help ? 0 : 2;
    return;
  }
  const archive = path.resolve(options.archive);
  const server = path.join(archive, "serve.py");
  if (!fs.existsSync(server)) {
    throw new Error("not a built archive (no serve.py): " + archive);
  }

  // A FRESH server per level, not one shared across the run. The reason that matters for the
  // numbers is the HTTP cache: a shared server let level N load from level N-1's warm cache, so
  // the later rows measured a state a real reader would not have and the rows were not
  // comparable with each other.
  async function measure(level) {
    const port = await unusedPort();
    // `detached`, so the server can be killed as a process GROUP. A bare SIGTERM to the pid was
    // observed leaving servers alive -- four at once during a four-level run that should never
    // have had more than one.
    const child = childProcess.spawn("python3", [server, "--port", String(port)], {
      cwd: archive,
      stdio: ["ignore", "pipe", "pipe"],
      detached: true
    });
    try {
      await waitForServer(port, 30_000);
      return runBenchmark("http://127.0.0.1:" + port + "/", level, options.timeoutMs);
    } finally {
      await stopGroup(child);
      // Let the port and the browser teardown settle before the next level starts.
      await new Promise(function (resolve) { setTimeout(resolve, 3_000); });
    }
  }

  const reports = [];
  for (const level of options.levels) {
    process.stderr.write("  measuring zoom=" + level + "...\n");
    let report;
    let retried = false;
    try {
      report = await measure(level);
    } catch (error) {
      // ONE retry, and it is disclosed in the output rather than laundering a failure into a
      // pass. See the note above `LEVELS` for exactly what is and is not understood about the
      // failure this exists for.
      process.stderr.write(
        "  zoom=" + level + " failed, retrying once: " +
        String(error && error.message).split("\n")[0] + "\n"
      );
      retried = true;
      report = await measure(level);
    }
    reports.push({ level: level, report: report, retried: retried });
  }

  const table = renderTable(reports);
  process.stdout.write(table);
  if (options.jsonPath) {
    fs.writeFileSync(
      options.jsonPath,
      JSON.stringify({ archive: archive, levels: reports }, null, 2) + "\n",
      "utf8"
    );
  }
  fs.writeFileSync(path.join(__dirname, "qa-archive-report.txt"), table + "\n", "utf8");
}

main().catch(function (error) {
  process.stderr.write("qa-archive: " + (error && error.message ? error.message : error) + "\n");
  process.exitCode = 1;
});

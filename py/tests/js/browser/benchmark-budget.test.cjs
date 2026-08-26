"use strict";

// Zoom and pan redraw latency, as a PASS/FAIL check that also reports what it measured.
//
// `benchmark-site.test.cjs` beside this file asserts that the harness *records* metrics -- that
// every field is present and non-negative. That is a check on the instrument. This is the check
// on the thing being measured: it drives the same twelve-step wheel sequence (four zoom-in, two
// pan-right, two pan-left, four zoom-out) and fails when a redraw takes longer than the budget.
//
// Both halves are needed and they are not redundant. A harness that silently stopped sampling
// would keep passing here -- every latency under budget, because there are none -- which is why
// the sample count is asserted too, and why the instrument test exists separately.
//
// WHY A BUDGET AT ALL, given the numbers vary by machine. Because "the page still renders" is
// not the property anyone cares about for a timeline: it is an interactive visualization, and a
// zoom that takes two seconds is broken in a way no functional assertion notices. A regression
// that made every redraw ten times slower would pass all thirty-five interaction tests.
//
// WHY THESE NUMBERS. They are order-of-magnitude ceilings, not targets, chosen the way the
// deadline in `test_supervisor_crash.py` was: to tell a REGRESSION from a slow machine, not fast
// from slow. Measured p95 on this fixture is a few milliseconds; the budget is 400ms, which is
// roughly two orders of magnitude of headroom. A CI box under load, a cold Chromium, and a
// debug build together do not approach it -- and a change that doubles redraw cost will not trip
// it either, which is the honest cost of a bound that must not flake. What it catches is the
// change that makes interaction qualitatively unusable, and that is worth a gate.
//
// The measured values are PRINTED on every run, pass or fail. A budget that only speaks when it
// is breached gives you no way to see a trend approaching it.
//
// ONE METRIC IS PRINTED BUT NOT ENFORCED. `handler_to_raf_ms` -- the page's own handler time,
// excluding the browser's input plumbing -- measures as NEGATIVE on this fixture (about -0.1ms),
// because the harness captures the rAF timestamp before the handler is observed to finish. A
// budget over a quantity that reads below zero is a gate that cannot fail, which is worse than
// no gate: it looks like coverage. It stays in the printed table because the trend is still
// informative, and it gets no assertion until the measurement is fixed.

const assert = require("node:assert/strict");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const util = require("node:util");

const execFile = util.promisify(childProcess.execFile);
const benchmarkPath = path.join(__dirname, "benchmark-site.cjs");
const fixtureServerPath = path.join(__dirname, "fixture-server.cjs");

//: Wall-clock ceilings, in milliseconds. See the header for why they are this loose.
const BUDGET = Object.freeze({
  // The gap from the wheel event to the frame that reflects it. This is "redraw speed" as a user
  // experiences it, and it is the number this file exists to defend.
  input_to_raf_p95_ms: 400,
  input_to_raf_max_ms: 800,
  // First usable frame. Generous: it includes Chromium startup on a cold cache.
  usable_ms: 10_000
});

//: The wheel sequence is twelve steps; a run that sampled fewer measured something else.
const EXPECTED_SAMPLES = 12;

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

async function waitForFixture(server) {
  return new Promise(function (resolve, reject) {
    let output = "";
    const timeout = setTimeout(function () {
      reject(new Error("fixture server did not become ready; output: " + output));
    }, 10_000);
    function finish(error) {
      clearTimeout(timeout);
      server.stdout.removeListener("data", onData);
      server.removeListener("exit", onExit);
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    }
    function onData(chunk) {
      output += chunk.toString("utf8");
      if (output.includes("timeline browser fixture:")) {
        finish(null);
      }
    }
    function onExit(code) {
      finish(new Error("fixture server exited before readiness with status " + code));
    }
    server.stdout.on("data", onData);
    server.once("exit", onExit);
  });
}

function reportMeasurements(report) {
  const input = report.interaction.input_to_raf_ms;
  const handler = report.interaction.handler_to_raf_ms;
  const rows = [
    ["first usable frame", report.timings.usable_ms, BUDGET.usable_ms],
    ["zoom/pan input->frame p50", input.p50, null],
    ["zoom/pan input->frame p95", input.p95, BUDGET.input_to_raf_p95_ms],
    ["zoom/pan input->frame max", input.max, BUDGET.input_to_raf_max_ms],
    ["page handler->frame p95", handler.p95, null]
  ];
  const lines = rows.map(function (row) {
    const budget = row[2] === null ? "" : "  (budget " + row[2] + "ms)";
    return "    " + row[0].padEnd(28) + String(row[1]).padStart(8) + "ms" + budget;
  });
  // Written to a file as well as printed, and the file is the load-bearing half. Under
  // `dagrun box` -- which is how this runs in `make validate` -- a passing step's stdout is
  // summarised to one line, so a table that only went to the console would be visible exactly
  // when nobody needed it and swallowed the rest of the time. The file also gives the numbers
  // somewhere to accumulate: a budget tells you when you have already lost, a series tells you
  // that you are heading there.
  const table = [
    "timeline redraw benchmark",
    "sequence: " + report.interaction.sequence,
    ...rows.map(function (row) {
      const budget = row[2] === null ? "" : "  (budget " + row[2] + "ms)";
      return "  " + row[0].padEnd(28) + String(row[1]).padStart(8) + "ms" + budget;
    })
  ].join("\n") + "\n";
  fs.writeFileSync(path.join(__dirname, "benchmark-report.txt"), table, "utf8");

  // Printed on every run, not only on failure: a number you can only see when it is already too
  // late is not a measurement, it is an alarm.
  console.log(
    [
      "",
      "  timeline redraw benchmark (" + report.interaction.sequence + ")",
      ...lines,
      "    " + "frames that changed".padEnd(28) +
        String(report.interaction.render_change_count).padStart(8) + "/" +
        report.interaction.sample_count,
      "    " + "DOM / SVG nodes".padEnd(28) +
        String(report.timeline.final.counts.dom_nodes) + " / " +
        String(report.timeline.final.counts.svg_nodes),
      "    " + "JS heap".padEnd(28) +
        String(Math.round(report.js_heap.cdp.used_js_heap_bytes / 1048576)).padStart(8) + "MiB",
      ""
    ].join("\n")
  );
}

test("zoom and pan redraw within budget, and the measurements are reported", {
  timeout: 120_000
}, async function (context) {
  const port = await unusedPort();
  const server = childProcess.spawn(
    process.execPath,
    [fixtureServerPath, "--port", String(port)],
    { cwd: __dirname, stdio: ["ignore", "pipe", "pipe"] }
  );
  context.after(function () {
    if (!server.killed) {
      server.kill("SIGTERM");
    }
  });
  await waitForFixture(server);

  const temporaryDirectory = fs.mkdtempSync(
    path.join(os.tmpdir(), "agent-timeline-budget-")
  );
  context.after(function () {
    fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  });
  const jsonPath = path.join(temporaryDirectory, "benchmark.json");
  const result = await execFile(
    process.execPath,
    [
      benchmarkPath,
      "--url", "http://127.0.0.1:" + port + "/",
      "--json", jsonPath,
      "--timeout-ms", "30000"
    ],
    { cwd: __dirname, maxBuffer: 5 * 1024 * 1024 }
  );
  const report = JSON.parse(result.stdout);

  reportMeasurements(report);

  assert.equal(report.success, true, "the page raised a runtime error while being measured");
  assert.deepEqual(report.diagnostics.page_errors, []);

  // A harness that stopped sampling would otherwise pass every budget below by measuring nothing.
  assert.equal(
    report.interaction.sample_count,
    EXPECTED_SAMPLES,
    "the wheel sequence did not produce the expected number of samples"
  );
  assert.equal(report.interaction.input_to_raf_ms.count, EXPECTED_SAMPLES);

  const input = report.interaction.input_to_raf_ms;
  assert.ok(
    input.p95 <= BUDGET.input_to_raf_p95_ms,
    `zoom/pan p95 redraw was ${input.p95}ms, over the ${BUDGET.input_to_raf_p95_ms}ms budget`
  );
  assert.ok(
    input.max <= BUDGET.input_to_raf_max_ms,
    `slowest zoom/pan redraw was ${input.max}ms, over the ${BUDGET.input_to_raf_max_ms}ms budget`
  );
  assert.ok(
    report.timings.usable_ms <= BUDGET.usable_ms,
    `first usable frame took ${report.timings.usable_ms}ms, over the ${BUDGET.usable_ms}ms budget`
  );
});

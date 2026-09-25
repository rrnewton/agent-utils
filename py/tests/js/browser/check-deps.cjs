#!/usr/bin/env node
// Refuse, at once and by name, to run the browser or benchmark suite without its installed
// dependency. `npx playwright test` in a directory with no `node_modules/` does not fail: it asks
// the npm registry for a `playwright` to download. From a host that cannot reach the registry,
// that held a validation run for 481 seconds (three ETIMEDOUT attempts), after which npx fell back
// to a stale cached `playwright` of another version, which then failed to load `@playwright/test`
// anyway. Installing is a deliberate, networked step taken once; running the tests never fetches.
//
// Checks the directory's own `node_modules/` (the tests are run from its `.bin/`) for exactly the
// version `package.json` pins, since any other version was not installed from this manifest.

"use strict";

const fs = require("fs");
const path = require("path");

const PACKAGE = "@playwright/test";
const wanted = (require("./package.json").devDependencies || {})[PACKAGE];
const manifest = path.join(__dirname, "node_modules", PACKAGE, "package.json");
const runner = path.join(__dirname, "node_modules", ".bin", "playwright");

const problems = [];
if (wanted === undefined) problems.push(`package.json does not pin ${PACKAGE} in devDependencies`);
let installed = null;
try {
  installed = JSON.parse(fs.readFileSync(manifest, "utf8")).version;
} catch (error) {
  problems.push(
    error.code === "ENOENT"
      ? `${PACKAGE} is not installed in ${path.dirname(manifest)}`
      : `${manifest} cannot be read: ${error.message}`,
  );
}
if (installed !== null && wanted !== undefined && installed !== wanted) {
  problems.push(`${PACKAGE} ${installed} is installed; package.json pins ${wanted}`);
}
if (!fs.existsSync(runner)) problems.push(`${runner} does not exist`);

if (problems.length > 0) {
  process.stderr.write(
    [
      "wrkviz browser dependencies are not installed, so the suite cannot run:",
      ...problems.map((problem) => `  - ${problem}`),
      "Install them once. From this host's npm cache, with no network:",
      `  cd ${__dirname}`,
      "  npm install --offline --no-audit --no-fund --no-package-lock",
      "Or, if that reports ENOTCACHED, from the npm registry (on a proxied host prefix with-proxy):",
      "  npm install --no-audit --no-fund --no-package-lock",
      "and, if Playwright then reports a missing browser executable:",
      "  npx playwright install chromium",
      "The suite does not fetch them itself: an unreachable registry turns that into a long hang.",
      "",
    ].join("\n"),
  );
  process.exit(2);
}
process.stdout.write(`wrkviz browser dependencies: ${PACKAGE} ${installed}\n`);

// Every colour in the page comes from the theme (`#2155 vibe-talk-theme-settings-pane`,
// owner 342): a hard-coded colour outside the token definitions fails this check. The
// token definitions themselves live in the scheme/preset/config tables and the `:root`
// custom-property declarations; the pre-paint script mirrors them in the shell.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const WEB = join(dirname(fileURLToPath(import.meta.url)), "../../../web");

// Files whose whole job is holding the token tables: literals are definitions there.
const TOKEN_FILES = new Set([
  "src/core/settings/appearance.ts",
  "src/core/settings/colour.ts",
  "src/core/settings/themeconfig.ts",
]);

// Explicit allow-list, each with its reason. Anything else must become a token.
// - Per-thread / per-summary identity hues: the hue is data (which thread), the
//   saturation/lightness come from `--thread-sat`/`--thread-light` tokens.
const ALLOW = [
  { snippet: "hsl(var(--thread-hue", reason: "thread identity hue is data; sat/light are tokens" },
  { snippet: "hsl(var(--summary-hue", reason: "summary identity hue is data; sat/light are tokens" },
  { snippet: "mask-image: linear-gradient(to bottom, transparent 0, #000", reason: "mask geometry, not a colour" },
  { snippet: "-webkit-mask-image: linear-gradient(to bottom, transparent 0, #000", reason: "mask geometry, not a colour" },
];

const COLOUR = /#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(/;

function stripComments(text, isCss) {
  let out = text.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, " "));
  out = out.split("\n").map((line) => line.replace(/\(#\d+\)/g, "(issue)")).join("\n"); // (#1234) citations are not colours
  if (!isCss) out = out.replace(/(^|[^:])\/\/[^\n]*/g, (m, p) => p + " ".repeat(m.length - p.length));
  return out;
}

function* filesUnder(dir, suffixes) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* filesUnder(path, suffixes);
    else if (suffixes.some((s) => entry.name.endsWith(s))) yield path;
  }
}

test("`#2155`: no hard-coded colour outside the theme token definitions", () => {
  const violations = [];
  const targets = [
    join(WEB, "style.css"),
    ...filesUnder(join(WEB, "styles"), [".css"]),
    ...filesUnder(join(WEB, "src"), [".ts", ".js"]),
  ];
  for (const path of targets) {
    const rel = path.slice(WEB.length + 1);
    if (TOKEN_FILES.has(rel)) continue;
    const isCss = rel.endsWith(".css");
    const lines = stripComments(readFileSync(path, "utf8"), isCss).split("\n");
    lines.forEach((line, index) => {
      if (!COLOUR.test(line)) return;
      if (isCss && /^\s*--[a-z0-9-]+\s*:/i.test(line)) return; // a token definition
      if (/var\(--[a-z0-9-]+\)/.test(line) && !/#[0-9a-fA-F]{3,8}\b/.test(line)) return; // token-driven formula
      if (ALLOW.some((a) => line.includes(a.snippet))) return;
      violations.push(`${rel}:${index + 1}: ${line.trim()}`);
    });
  }
  // The shell's pre-paint script is a token mirror, not a use site: only there may
  // voice.html carry literals, and the lint above does not scan it.
  assert.deepStrictEqual(violations, [], `hard-coded colours (make them theme tokens):\n${violations.join("\n")}`);
});

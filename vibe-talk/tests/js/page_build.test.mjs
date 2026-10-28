// The page build (web/build.mjs): what it writes, and that what it writes is the page.
//
// `#624 voice-page-modularity`. The server embeds this build's output, so a fault here is a fault in
// every page served. These check the properties the server and the browser rely on: each hashed name
// is its bytes' hash; the shell differs from the source shell in its asset tags and nowhere else;
// the minified script keeps every top-level name and its strictness; a source map resolves a
// position in the minified file to the right line of the right source; nothing names the machine it
// was built on; two builds of the same sources are the same bytes.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { SourceMap } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { build, decodeMappings, digest, encodeMappings, joinMaps, requireStrict } from "../../web/build.mjs";
import { SHELL, WEB, readWeb, shellScripts, shellStyles } from "../../web/page_sources.mjs";

const scratch = mkdtempSync(join(tmpdir(), "vibe-talk-page-build-"));
process.on("exit", () => rmSync(scratch, { recursive: true, force: true }));

const OUT = join(scratch, "web");
const MANIFEST = await build(OUT);
const read = (path) => readFileSync(join(OUT, path));
const text = (path) => read(path).toString("utf8");
const relativePath = (path) => path.replace(/^\//, "");
const SCRIPT_PATH = relativePath(MANIFEST.assets.script.path);
const STYLE_PATH = relativePath(MANIFEST.assets.style.path);

/** Every file below `root`, as a path relative to it, sorted. */
const tree = (root, prefix = "") =>
  readdirSync(join(root, prefix)).sort().flatMap((name) => {
    const path = prefix ? `${prefix}/${name}` : name;
    return statSync(join(root, path)).isDirectory() ? tree(root, path) : [path];
  });

test("each hashed file is named by the hash of its bytes, and the page build is the script's", () => {
  for (const asset of [MANIFEST.assets.script, MANIFEST.assets.style]) {
    for (const path of [asset.path, asset.map]) {
      const found = /^\/assets\/voice\.([0-9a-f]{32})\.(?:js|css)(?:\.map)?$/.exec(path);
      assert.ok(found, `${path} is not an assets/voice.<hash> name`);
      assert.equal(digest(read(relativePath(path))), found[1], `${path} does not hold the bytes its name says`);
    }
  }
  assert.equal(MANIFEST.page_build, digest(read(SCRIPT_PATH)));
  assert.deepStrictEqual(tree(OUT).filter((path) => path.startsWith("assets/")).sort(),
    [SCRIPT_PATH, relativePath(MANIFEST.assets.script.map), STYLE_PATH, relativePath(MANIFEST.assets.style.map)].sort(),
    "assets/ holds something besides the two files and their maps");
});

test("assets.json's digests are the files as written", () => {
  for (const [path, want] of Object.entries(MANIFEST.files)) assert.equal(digest(read(path)), want, path);
  assert.deepStrictEqual(JSON.parse(text("assets.json")), MANIFEST);
  assert.equal(MANIFEST.tools.esbuild, "0.28.2", "the build ran another esbuild than the lockfile pins");
});

test("the shell is the source shell with its script tags made one and its stylesheet links made one, and nothing else changed", () => {
  const source = readWeb(SHELL);
  const scripts = shellScripts(source);
  const styles = shellStyles(source);
  assert.deepStrictEqual(MANIFEST.assets.script.sources, scripts);
  assert.deepStrictEqual(MANIFEST.assets.style.sources, styles);
  let expected = source;
  scripts.forEach((name, i) => {
    expected = expected.replace(new RegExp(`( *)<script src="/${name}"></script>\\n`),
      (_, indent) => (i === 0 ? `${indent}<script src="/${SCRIPT_PATH}"></script>\n` : ""));
  });
  styles.forEach((name, i) => {
    expected = expected.replace(new RegExp(`( *)<link rel="stylesheet" href="/${name}" />\\n`),
      (_, indent) => (i === 0 ? `${indent}<link rel="stylesheet" href="/${STYLE_PATH}" />\n` : ""));
  });
  assert.notEqual(expected, source);
  assert.equal(text(SHELL), expected);
  assert.deepStrictEqual(shellScripts(text(SHELL)), [SCRIPT_PATH]);
  assert.deepStrictEqual(shellStyles(text(SHELL)), [STYLE_PATH]);
});

test("the plain addresses an older shell names: /voice.js the whole new script, the rest as written", () => {
  assert.deepStrictEqual(read("voice.js"), read(SCRIPT_PATH));
  for (const name of ["contract.js", "style.css", "voice.css", "manifest.webmanifest"]) {
    assert.deepStrictEqual(read(name), readFileSync(join(WEB, name)), name);
  }
  for (const name of readdirSync(join(WEB, "icons"))) {
    assert.deepStrictEqual(read(`icons/${name}`), readFileSync(join(WEB, "icons", name)), name);
  }
});

test("the script is strict throughout, keeps every top-level name, and links its map", () => {
  const script = text(SCRIPT_PATH);
  assert.ok(script.startsWith('"use strict";\n'), "the joined script does not open in strict mode");
  const mapName = MANIFEST.assets.script.map.split("/").pop();
  assert.ok(script.endsWith(`\n//# sourceMappingURL=${mapName}\n`), "the script does not name its map last");
  for (const name of shellScripts(readWeb(SHELL))) {
    const source = readWeb(name);
    // A script wrapped in one function has nothing at the top level but the wrapper (contract.js);
    // its column-0 lines are the function's body.
    if (/^\(function \(\) \{/m.test(source.replace(/^(?:\/\/[^\n]*\n)*/, ""))) continue;
    const declared = [...source.matchAll(/^(?:async )?function\*? ?([A-Za-z_$][\w$]*)\(|^(?:const|let|var) ([A-Za-z_$][\w$]*)\b/gm)]
      .map((m) => m[1] || m[2]);
    for (const top of declared) {
      assert.match(script, new RegExp(`(?:function\\*? ?|const |let |var |,)${top.replace(/\$/g, "\\$")}[=(,;]`),
        `${name}'s top-level ${top} is not declared under its own name in the bundle`);
    }
  }
  assert.ok(text(STYLE_PATH).endsWith(`/*# sourceMappingURL=${MANIFEST.assets.style.map.split("/").pop()} */\n`));
});

test("a position in the minified files maps back to the line of the source it came from", () => {
  const probes = [
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "const TOKEN_KEY="],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "const PAGE_BUILD="],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "function fmtDuration("],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "contract.js", "function decodeTagged("],
    [STYLE_PATH, MANIFEST.assets.style.map, "voice.css", ".send-tick{"],
    [STYLE_PATH, MANIFEST.assets.style.map, "style.css", "body{"],
  ];
  for (const [asset, mapPath, file, needle] of probes) {
    const lines = text(asset).split("\n");
    const line = lines.findIndex((l) => l.includes(needle));
    assert.ok(line >= 0, `${needle} is not in ${asset}`);
    const map = new SourceMap(JSON.parse(text(relativePath(mapPath))));
    const found = map.findEntry(line, lines[line].indexOf(needle));
    const source = readWeb(file);
    const sourceNeedle = needle.replace(/=$/, " =").replace(/\{$/, " {");
    const want = source.slice(0, source.indexOf(sourceNeedle)).split("\n").length - 1;
    assert.ok(source.includes(sourceNeedle), `${sourceNeedle} is not in ${file}`);
    assert.equal(found.originalSource, `vibe-talk/web/${file}`, needle);
    assert.equal(found.originalLine, want, `${needle} maps to line ${found.originalLine + 1}, not ${want + 1}`);
  }
});

test("nothing built names the directory it was built in or the one it was built from", () => {
  for (const path of tree(OUT)) {
    const bytes = read(path).toString("latin1");
    for (const local of [WEB, OUT, scratch]) assert.ok(!bytes.includes(local), `${path} names ${local}`);
  }
  for (const map of [MANIFEST.assets.script.map, MANIFEST.assets.style.map]) {
    const { sources, sourcesContent } = JSON.parse(text(relativePath(map)));
    for (const source of sources) assert.match(source, /^vibe-talk\/web\/[\w.-]+$/, `${map} names ${source}`);
    assert.deepStrictEqual(sourcesContent, sources.map((source) => readWeb(source.replace("vibe-talk/web/", ""))));
  }
});

test("two builds of the same sources are the same bytes, and a rebuild replaces the directory whole", async () => {
  const again = join(scratch, "again");
  await build(again);
  assert.deepStrictEqual(tree(again), tree(OUT));
  for (const path of tree(OUT)) assert.deepStrictEqual(readFileSync(join(again, path)), read(path), path);
  await build(again);
  assert.deepStrictEqual(tree(again), tree(OUT), "a second build into the same directory left or lost a file");
  assert.deepStrictEqual(readdirSync(scratch).filter((name) => name.includes(".tmp-") || name.includes(".old-")), [],
    "a build left its staging directory behind");
});

test("joined maps keep each part's positions, its sources and its names", () => {
  // Two parts, two lines and one line, behind one leading line: the second part's first segment
  // lands on generated line 3 and points into ITS source, at ITS line.
  const first = { version: 3, sources: ["a.js"], sourcesContent: ["A"], names: ["x"],
    mappings: encodeMappings([[[0, 0, 0, 0], [4, 0, 1, 2, 0]], [[1, 0, 5, 0]]]) };
  const second = { version: 3, sources: ["b.js"], sourcesContent: ["B"], names: ["y"],
    mappings: encodeMappings([[[2, 0, 7, 3, 0]]]) };
  const joined = joinMaps([{ lineCount: 2, map: first }, { lineCount: 1, map: second }], 1);
  assert.deepStrictEqual(joined.sources, ["a.js", "b.js"]);
  assert.deepStrictEqual(joined.names, ["x", "y"]);
  assert.deepStrictEqual(decodeMappings(joined.mappings),
    [[], [[0, 0, 0, 0], [4, 0, 1, 2, 0]], [[1, 0, 5, 0]], [[2, 1, 7, 3, 1]]]);
  assert.throws(() => joinMaps([{ lineCount: 1, map: first }], 0), /covers 2 lines of its 1/);
  // Large and negative deltas survive the round trip.
  const wide = [[[0, 0, 40000, 7]], [[3, 0, 2, 1]], [], [[123456, 0, 99999, 0]]];
  assert.deepStrictEqual(decodeMappings(encodeMappings(wide)), wide);
});

test("a script that is not strict throughout is refused rather than made strict by the join", () => {
  requireStrict("ok.js", '// a comment\n"use strict";\nlet a = 1;\n');
  requireStrict("wrapped.js", '// generated\n(function () {\n"use strict";\nconst a = 1;\n})();\n');
  assert.throws(() => requireStrict("sloppy.js", "let a = 1;\n"), /sloppy\.js is not strict throughout/);
  assert.throws(() => requireStrict("late.js", 'let a = 1;\n"use strict";\n'), /late\.js is not strict/);
  assert.throws(() => requireStrict("tail.js", '(function () {\n"use strict";\n})();\nwindow.x = 1;\n'), /tail\.js is not strict/);
});

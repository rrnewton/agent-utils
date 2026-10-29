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
import { cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { SourceMap } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import vm from "node:vm";

import {
  build, decodeMappings, digest, encodeMappings, joinMaps, linkShell, loadTool, moduleForTests, parseScript, requireOutside,
  requireStrict,
} from "../../web/build.mjs";
import { SHELL, WEB, moduleSources, readWeb, shellScripts, shellStyles } from "../../web/page_sources.mjs";

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

/** The names a script declares at its top level, by the pinned TypeScript parser. */
function topLevelNames(name, text) {
  const { ts, file } = parseScript(name, text);
  const names = [];
  const bound = (node) => {
    if (ts.isIdentifier(node)) names.push(node.text);
    else for (const element of node.elements) if (!ts.isOmittedExpression(element)) bound(element.name);
  };
  for (const statement of file.statements) {
    if (ts.isVariableStatement(statement)) statement.declarationList.declarations.forEach((d) => bound(d.name));
    else if ((ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement)) && statement.name) {
      names.push(statement.name.text);
    }
  }
  return names;
}

test("the script is strict throughout, declares every source's top-level name at its own top level, and links its map", () => {
  const script = text(SCRIPT_PATH);
  assert.ok(script.startsWith('"use strict";\n'), "the joined script does not open in strict mode");
  const declared = new Set(topLevelNames("bundle.js", script));
  let checked = 0;
  for (const name of shellScripts(readWeb(SHELL))) {
    for (const top of topLevelNames(name, readWeb(name))) {
      assert.ok(declared.has(top), `${name}'s top-level ${top} is not declared at the bundle's top level`);
      checked += 1;
    }
  }
  assert.ok(checked > 1000, `only ${checked} top-level names were checked`);
  const mapPath = MANIFEST.assets.script.map;
  assert.ok(script.endsWith(`\n//# sourceMappingURL=${mapPath}\n`), "the script does not name its map last");
  assert.ok(text(STYLE_PATH).endsWith(`/*# sourceMappingURL=${MANIFEST.assets.style.map} */\n`));
});

test("an error the page throws is reported at the line of its source it was thrown from", () => {
  // Run with no browser at all, both the sources and the bundle stop on the same missing global, at
  // the same statement: the bundle's stack, mapped back, must name the line the sources name.
  const thrown = (code, filename) => {
    try {
      new vm.Script(code, { filename }).runInContext(vm.createContext({}));
    } catch (error) {
      const at = new RegExp(`${filename.replace(/\./g, "\\.")}:(\\d+):(\\d+)`).exec(error.stack);
      assert.ok(at, `no frame in ${filename}: ${error.stack}`);
      return { message: error.message, line: Number(at[1]) - 1, column: Number(at[2]) - 1 };
    }
    return assert.fail(`${filename} ran to the end without a browser`);
  };
  const scripts = shellScripts(readWeb(SHELL));
  const last = scripts.at(-1);
  // The sources as the shell loads them: every script before the last runs cleanly, and the last
  // throws.
  const context = vm.createContext({});
  for (const name of scripts.slice(0, -1)) new vm.Script(readWeb(name), { filename: name }).runInContext(context);
  let fromSource;
  try {
    new vm.Script(readWeb(last), { filename: last }).runInContext(context);
  } catch (error) {
    const at = new RegExp(`${last.replace(/\./g, "\\.")}:(\\d+):`).exec(error.stack);
    fromSource = { message: error.message, line: Number(at[1]) - 1 };
  }
  assert.ok(fromSource, `${last} ran to the end without a browser`);
  const fromBundle = thrown(text(SCRIPT_PATH), "bundle.js");
  assert.equal(fromBundle.message, fromSource.message);
  const map = new SourceMap(JSON.parse(text(relativePath(MANIFEST.assets.script.map))));
  const found = map.findEntry(fromBundle.line, fromBundle.column);
  assert.equal(found.originalSource, `vibe-talk/web/${last}`);
  assert.equal(found.originalLine, fromSource.line,
    `the bundle's ${fromBundle.message} maps to line ${found.originalLine + 1}, not ${fromSource.line + 1}`);
});

test("a position in the minified files maps back to the line of the source it came from", () => {
  const probes = [
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "const TOKEN_KEY="],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "const PAGE_BUILD="],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "voice.js", "function fmtDuration("],
    [SCRIPT_PATH, MANIFEST.assets.script.map, "src/contract/validators.js", "function decodeTagged("],
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

test("the shell is refused rather than mis-linked when its tags are not the plain ones the build replaces", () => {
  const shell = (extra) => `<head>\n<link rel="stylesheet" href="/a.css" />\n</head><body>\n${extra}<script src="/a.js"></script>\n</body>`;
  assert.equal(linkShell(shell(""), "/assets/x.js", "/assets/x.css"),
    '<head>\n<link rel="stylesheet" href="/assets/x.css" />\n</head><body>\n<script src="/assets/x.js"></script>\n</body>');
  for (const extra of ['<!--\n<script src="/old.js"></script>\n-->\n', '<script src="/b.js" defer></script>\n',
    '<link rel="stylesheet" href="/print.css" media="print" />\n', '<script src="/b.js"></script><script src="/c.js"></script>\n']) {
    assert.throws(() => linkShell(shell(extra), "/assets/x.js", "/assets/x.css"), /not the plain, one-per-line tags/, extra);
  }
});

test("a copy of the sources changed on purpose builds the same way, and is refused as an output too", async () => {
  // A browser walk's negative control: the page with one thing broken, built exactly as the page is.
  const copy = join(scratch, "patched-sources");
  cpSync(WEB, copy, { recursive: true });
  const voice = join(copy, "voice.js");
  writeFileSync(voice, readFileSync(voice, "utf8").replace("const TOKEN_KEY = ", "const TOKEN_KEY_PATCHED = "));
  const out = join(scratch, "patched");
  const patched = await build(out, { web: copy });
  assert.notEqual(patched.page_build, MANIFEST.page_build);
  assert.match(readFileSync(join(out, relativePath(patched.assets.script.path)), "utf8"), /const TOKEN_KEY_PATCHED=/);
  assert.notEqual(patched.inputs["vibe-talk/web/voice.js"], MANIFEST.inputs["vibe-talk/web/voice.js"]);
  assert.throws(() => requireOutside(join(copy, "out"), copy), /inside/);
});

test("the build refuses to publish over its own sources or the repository", () => {
  for (const out of [WEB, join(WEB, ".."), join(WEB, "..", ".."), join(WEB, "dist"), join(WEB, "..", "contract")]) {
    assert.throws(() => requireOutside(out), /would (replace|become an input)/, out);
  }
  requireOutside(join(WEB, "..", "target", "web"));
  requireOutside(OUT);
});

test("nothing built names the directory it was built in or the one it was built from", () => {
  for (const path of tree(OUT)) {
    const bytes = read(path).toString("latin1");
    for (const local of [WEB, OUT, scratch]) assert.ok(!bytes.includes(local), `${path} names ${local}`);
  }
  for (const map of [MANIFEST.assets.script.map, MANIFEST.assets.style.map]) {
    const { sources, sourcesContent } = JSON.parse(text(relativePath(map)));
    for (const source of sources) {
      assert.match(source, /^vibe-talk\/web\/[\w.-]+(\/[\w.-]+)*$/, `${map} names ${source}`);
      assert.ok(!source.split("/").includes(".."), `${map} names ${source}`);
    }
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
  const ts = loadTool("typescript");
  requireStrict("ok.js", '// a comment\n/* another */\n"use strict";\nlet a = 1;\n', ts);
  requireStrict("prologue.js", '"a directive";\n\'use strict\';\nlet a = 1;\n', ts);
  requireStrict("wrapped.js", '// generated\n(function () {\n"use strict";\nconst a = 1;\n})();\n', ts);
  for (const [name, code] of [
    ["sloppy.js", "let a = 1;\n"],
    ["late.js", 'let a = 1;\n"use strict";\n'],
    ["escaped.js", '"use\\x20strict";\nlet a = 1;\n'],
    ["tail.js", '(function () {\n"use strict";\n})();\nwindow.x = 1;\n'],
    ["sandwich.js", '(function(){"use strict";})();\nwith (o) { x = 1; }\n(function(){})();\n'],
    ["argument.js", '(function (a) {\n"use strict";\n})(window);\n'],
    ["inner.js", '(function () {\nlet a = 1;\n"use strict";\n})();\n'],
  ]) {
    assert.throws(() => requireStrict(name, code, ts), new RegExp(`${name.replace(".", "\\.")} is not strict throughout`), name);
  }
  assert.throws(() => requireStrict("broken.js", '"use strict";\nlet = ;\n', ts), /broken\.js does not parse/);
});

// ---- The legacy prelude (`#624 voice-page-modularity`) ------------------------------------------

test("the prelude gives the legacy script exactly what web/src/legacy/imports.ts exports, and its declarations say the same", () => {
  const names = MANIFEST.legacy_imports;
  assert.ok(names.length > 0, "nothing has moved, so this checks nothing");
  const declared = [...readWeb("legacy-imports.d.ts").matchAll(/^declare const (\w+): LegacyImports\["(\w+)"\];$/gm)]
    .map(([, name, key]) => (assert.equal(name, key), name));
  // VibeTalkContract keeps the declaration web/contract.d.ts has always given it, for the global.
  assert.match(readWeb("contract.d.ts"), /^declare const VibeTalkContract: \{/m);
  const expected = names.filter((name) => name !== "VibeTalkContract");
  assert.ok(names.includes("VibeTalkContract"), "the legacy script no longer takes the contract from the prelude");
  assert.deepStrictEqual(declared.sort(), [...expected].sort(), "web/legacy-imports.d.ts and imports.ts name different things");
});

test("the bundle declares nothing of the modules but what the legacy script takes, strictly, just before it", () => {
  const script = text(SCRIPT_PATH);
  const legacy = topLevelNames("voice.js", readWeb("voice.js"));
  const allowed = new Set([...legacy, ...MANIFEST.legacy_imports]);
  const declared = topLevelNames("bundle.js", script);
  assert.deepStrictEqual(declared.filter((name) => !allowed.has(name)), [],
    "the bundle declares a module's name at its top level beyond the legacy imports");
  // The prelude's one declaration, opening a strict function, immediately before the legacy script.
  const { ts, file } = parseScript("bundle.js", script);
  const statements = file.statements;
  const at = statements.findIndex((statement) => ts.isVariableStatement(statement) &&
    ts.isObjectBindingPattern(statement.declarationList.declarations[0].name));
  assert.ok(at > 0, "no prelude declaration in the bundle");
  const prelude = statements[at];
  assert.deepStrictEqual(prelude.declarationList.declarations[0].name.elements.map((element) => element.name.text).sort(),
    [...MANIFEST.legacy_imports].sort());
  assert.ok(prelude.getText(file).includes('= (() => {\n"use strict";\n'), "the modules do not run in a strict function");
  const next = statements[at + 1].getText(file);
  assert.ok(next.startsWith('"use strict"'), `the legacy script does not follow the prelude: ${next.slice(0, 80)}`);
  // The modules' own names stay inside it.
  for (const internal of ["SEARCH_FOLD_TABLE", "searchTokens", "__voiceLegacyImports"]) {
    assert.ok(!declared.includes(internal), `${internal} is declared at the bundle's top level`);
  }
});

test("a position in the prelude maps back to the module's TypeScript source", () => {
  const lines = text(SCRIPT_PATH).split("\n");
  const needle = "function searchCombine(";
  const line = lines.findIndex((l) => l.includes(needle));
  assert.ok(line >= 0, `${needle} is not in the bundle`);
  const map = new SourceMap(JSON.parse(text(relativePath(MANIFEST.assets.script.map))));
  const found = map.findEntry(line, lines[line].indexOf(needle));
  const source = readWeb("src/core/search/language.ts");
  assert.equal(found.originalSource, "vibe-talk/web/src/core/search/language.ts");
  assert.equal(found.originalLine, source.slice(0, source.indexOf(needle)).split("\n").length - 1);
});

test("a module overlay whose text the module does not hold is refused, not built unchanged", async () => {
  await assert.rejects(moduleForTests("src/core/search/index.ts", {
    overlays: [{ file: "src/core/search/language.ts", find: "no such text anywhere", replace: "" }],
  }), /holds 0 times, not 1/);
  const patched = await moduleForTests("src/core/search/index.ts", {
    overlays: [{ file: "src/core/search/language.ts", find: "const SEARCH_MAX_UPSTREAM_QUERIES = 4;", replace: "const SEARCH_MAX_UPSTREAM_QUERIES = 1;" }],
  });
  const plan = patched.searchUpstreamPlan(patched.searchQueryParse("alpha OR bravo"));
  assert.equal(plan.scan, "too-many-alternatives", "the overlay did not reach the built module");
});

test("every handwritten module is TypeScript, so the strict and portable checks see all of them", () => {
  // Machine output may be JavaScript, with generated declarations beside it (the contract's
  // validators), as the plan allows; nothing written by hand may.
  const names = new Set(moduleSources().map(({ name }) => name));
  const plain = moduleSources()
    .filter(({ name, generated }) => !name.endsWith(".ts") && !(generated && names.has(name.replace(/\.js$/, ".d.ts"))))
    .map(({ name }) => name);
  assert.deepStrictEqual(plain, [], "a module below web/src is not .ts, so tsconfig.core.json and tsconfig.web.json skip it");
  assert.ok(moduleSources().length > 0, "no module found, so this checks nothing");
});

test("no module opts out of type checking: no @ts-nocheck, @ts-ignore or @ts-expect-error, and no `any`", () => {
  const found = [];
  for (const { name, text } of moduleSources().filter(({ generated }) => !generated)) {
    const code = text.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
    code.split("\n").forEach((line, at) => {
      if (/@ts-(?:nocheck|ignore|expect-error)\b/.test(line) || /(?::|<|\bas)\s*any\b/.test(line.replace(/\/\/.*$/, ""))) {
        found.push(`${name}:${at + 1}: ${line.trim()}`);
      }
    });
  }
  assert.deepStrictEqual(found, [], "a module turns type checking off where the plan holds it strict");
  // The scan sees an opt-out where there is one.
  assert.ok(/@ts-(?:nocheck|ignore|expect-error)\b/.test("// @ts-nocheck"));
  assert.ok(/(?::|<|\bas)\s*any\b/.test("const x: any = 1;") && /(?::|<|\bas)\s*any\b/.test("y as any"));
});

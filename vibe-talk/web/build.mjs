// Build the page the server embeds: one minified script and one stylesheet, each under a
// content-hashed name with a linked source map, and a shell that names them.
//
// `#624 voice-page-modularity`, steps 3 and 4 of its plan
// (ai_docs/transient/2026-10-28-voice-page-modularity/PLAN.md). This is the ONE definition of that
// build: Cargo's build script runs it, and so do the tests and the browser walks, each into a
// directory of its own.
//
//   node web/build.mjs --out DIR
//
// DIR receives, published all at once when every file is written and checked:
//
//   assets/voice.<hash>.js, .css          the page's scripts and stylesheets, minified, in the
//                                         order the shell loads them, one of each
//   assets/voice.<hash>.js.map, .css.map  their source maps, named by their own hashes
//   voice.html                            the shell, its script and stylesheet tags replaced by
//                                         one tag each naming the hashed files; nothing else moved
//   voice.js, contract.js, voice.css,     what the plain addresses an older shell asks for
//   style.css                             serve (see "Compatibility" below)
//   manifest.webmanifest, icons/          as written, so DIR is a complete page to serve
//   assets.json                           what was built from what: names, digests, inputs, tools
//
// A <hash> is the first 128 bits of the SHA-256 of the file's bytes, in lowercase hex: the digest
// src/http/assets.rs names every other file by. A map is hashed first, the script then carries a
// `sourceMappingURL` naming the map by its absolute path (so the copy at the plain /voice.js finds
// it too), and the script's name is the hash of THOSE bytes, so neither hash covers itself.
// `page_build` in assets.json is the script's hash.
//
// HOW THE FILES ARE MINIFIED: whitespace and comments only. The page's scripts are still classic
// scripts sharing one global scope, so no identifier may be renamed (every top-level name stays the
// global it was, every function keeps its `name`, and esbuild adds no helper to the global scope);
// and syntax minification is not taken either, because it inlines a primitive `const` into its
// uses, which turns a temporal-dead-zone ReferenceError into a value. Measured at the time: syntax
// minification would save 1.2% more of the brotli-compressed script and 0.3% of the stylesheet.
// Each script is minified on its own and they are joined in the shell's order under one leading
// "use strict": a script that is not itself strict throughout is refused rather than made strict
// by the join (`requireStrict`). The stylesheets are joined in the shell's order likewise.
//
// COMPATIBILITY. A shell already in a browser's cache names the plain /contract.js, /voice.js,
// /style.css and /voice.css. /voice.js is the new complete script (the validators run twice, which
// sets the same global twice); /contract.js stays the standalone validators, because a cached old
// /voice.js still needs them; the two stylesheets stay as written, in their own order.

import { createHash, randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath, pathToFileURL } from "node:url";

import { SHELL, WEB, shellScripts, shellStyles } from "./page_sources.mjs";

/** The lockfile-pinned toolchain: contract/package.json, installed by `npm ci --prefix contract`. */
const TOOLS = join(WEB, "..", "contract");
const REPOSITORY = resolve(WEB, "..", "..");

/** The first 128 bits of the SHA-256 of `bytes`, in lowercase hex. */
export const digest = (bytes) => createHash("sha256").update(bytes).digest("hex").slice(0, 32);

/** One of the lockfile-pinned tools (esbuild, typescript), or an error saying how to install it. */
export function loadTool(name) {
  const require = createRequire(join(TOOLS, "package.json"));
  try {
    return require(name);
  } catch (error) {
    throw new Error(
      `the page build needs the pinned ${name} in vibe-talk/contract/node_modules; ` +
        `from the repository root, install it with: npm ci --prefix vibe-talk/contract\n(${error.message})`
    );
  }
}

// ---- Source maps ----------------------------------------------------------------------------
//
// Each script and stylesheet is minified on its own, with its own map; the parts are then joined
// one after another, each starting on a new line, and so are their maps. A map's `mappings` is a
// list of generated lines, each a list of segments whose fields are base64 VLQ numbers, most of
// them relative to the previous segment's. Joining two is decoding both to absolute positions and
// encoding the whole again.

const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_VALUE = new Map([...BASE64].map((c, i) => [c, i]));

/** The segments of a `mappings` string, by generated line, every field absolute. */
export function decodeMappings(mappings) {
  const lines = [];
  const state = [0, 0, 0, 0, 0]; // column, source, line, column, name
  for (const text of mappings.split(";")) {
    const line = [];
    state[0] = 0;
    for (const segment of text.split(",")) {
      if (segment === "") continue;
      const fields = [];
      let value = 0;
      let shift = 0;
      for (const c of segment) {
        const digit = BASE64_VALUE.get(c);
        if (digit === undefined) throw new Error(`a source map holds ${JSON.stringify(c)}, which is not base64`);
        value += (digit & 31) << shift;
        if (digit & 32) {
          shift += 5;
        } else {
          fields.push(value & 1 ? -(value >>> 1) : value >>> 1);
          value = 0;
          shift = 0;
        }
      }
      if (![1, 4, 5].includes(fields.length)) throw new Error(`a source map segment has ${fields.length} fields`);
      const absolute = fields.map((field, i) => (state[i] += field));
      line.push(absolute);
    }
    lines.push(line);
  }
  return lines;
}

const vlq = (value) => {
  let rest = value < 0 ? (-value << 1) | 1 : value << 1;
  let out = "";
  do {
    let digit = rest & 31;
    rest >>>= 5;
    if (rest > 0) digit |= 32;
    out += BASE64[digit];
  } while (rest > 0);
  return out;
};

/** `lines` of absolute segments, as a `mappings` string. */
export function encodeMappings(lines) {
  const state = [0, 0, 0, 0, 0];
  return lines
    .map((line) => {
      state[0] = 0;
      return line
        .map((segment) => segment.map((field, i) => {
          const out = vlq(field - state[i]);
          state[i] = field;
          return out;
        }).join(""))
        .join(",");
    })
    .join(";");
}

/**
 * One map for `parts` joined in order, each `{ lines, map }`: `lines` generated lines of output,
 * `map` its source map. A part starts on the line after the last one's.
 */
export function joinMaps(parts, leadingLines) {
  const sources = [];
  const sourcesContent = [];
  const names = [];
  const lines = Array.from({ length: leadingLines }, () => []);
  for (const { lineCount, map } of parts) {
    const sourceBase = sources.length;
    const nameBase = names.length;
    sources.push(...map.sources);
    sourcesContent.push(...(map.sourcesContent || map.sources.map(() => null)));
    names.push(...(map.names || []));
    const decoded = decodeMappings(map.mappings);
    if (decoded.length > lineCount) throw new Error(`a part's map covers ${decoded.length} lines of its ${lineCount}`);
    for (let i = 0; i < lineCount; i++) {
      lines.push((decoded[i] || []).map((segment) => {
        const out = [...segment];
        if (out.length >= 4) out[1] += sourceBase;
        if (out.length === 5) out[4] += nameBase;
        return out;
      }));
    }
  }
  return { version: 3, sources, sourcesContent, names, mappings: encodeMappings(lines) };
}

// ---- The build --------------------------------------------------------------------------------

/** `text` parsed as a classic script, by the pinned TypeScript's parser; a syntax error throws. */
export function parseScript(name, text, ts = loadTool("typescript")) {
  const file = ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  const [problem] = file.parseDiagnostics || [];
  if (problem) throw new Error(`${name} does not parse: ${ts.flattenDiagnosticMessageText(problem.messageText, " ")}`);
  return { ts, file };
}

/** Whether `statements` open with a directive prologue holding "use strict", spelled as a directive. */
function strictPrologue(ts, file, statements) {
  for (const statement of statements) {
    if (!ts.isExpressionStatement(statement) || !ts.isStringLiteral(statement.expression)) return false;
    if (['"use strict"', "'use strict'"].includes(statement.expression.getText(file))) return true;
  }
  return false;
}

/**
 * A script is joined under one "use strict" only if it was already strict throughout: either its
 * own directive prologue says "use strict" (voice.js), or the whole script is ONE call of a function
 * whose prologue does (contract.js's generated wrapper), so nothing outside that function exists.
 */
export function requireStrict(name, text, ts = loadTool("typescript")) {
  const { file } = parseScript(name, text, ts);
  const statements = file.statements;
  const only = statements.length === 1 ? statements[0] : null;
  const call = only && ts.isExpressionStatement(only) && ts.isCallExpression(only.expression) ? only.expression : null;
  let callee = call ? call.expression : null;
  while (callee && ts.isParenthesizedExpression(callee)) callee = callee.expression;
  const wrapper = callee && call.arguments.length === 0 &&
    (ts.isFunctionExpression(callee) || ts.isArrowFunction(callee)) && callee.body && ts.isBlock(callee.body) &&
    strictPrologue(ts, file, callee.body.statements);
  if (!strictPrologue(ts, file, statements) && !wrapper) {
    throw new Error(`${name} is not strict throughout, so joining it under "use strict" would change it`);
  }
}

/** The name a page file carries in source maps and assets.json, wherever the checkout is. */
const sourceName = (name) => `vibe-talk/web/${name}`;

// ---- The legacy prelude ---------------------------------------------------------------------------
//
// `#624 voice-page-modularity`. The page's code moves out of web/voice.js into TypeScript modules
// below web/src/ a piece at a time. What is left of voice.js is still a classic script, and still
// calls what moved; it is NOT bundled as a module, because esbuild bundling a module rewrites its
// top-level `let`/`const`/`class` to `var` (a read before its declaration would then give
// `undefined` instead of throwing) and drops its "use strict" (checked against esbuild 0.28.2). So
// the modules it calls are bundled on their own, inside a strict function, and that function hands
// the legacy script exactly the names web/src/legacy/imports.ts exports, as constants declared just
// before it runs:
//
//   const { searchQueryParse, ... } = (() => {
//   "use strict";
//   var __voiceLegacyImports = (() => { ...the modules... })();
//   return __voiceLegacyImports;
//   })();
//
// No other name the modules use is visible outside them, and no object is left behind on the page.

/** The legacy page script, which the prelude runs just before. */
export const LEGACY_SCRIPT = "voice.js";
/** The module whose exports are what the legacy script takes from the modules. */
export const LEGACY_IMPORTS = "src/legacy/imports.ts";
const PRELUDE_GLOBAL = "__voiceLegacyImports";

/**
 * An esbuild plugin that loads each file `overlays` names with its patch applied: `{ file, find,
 * replace, count = 1 }`, `file` relative to `web`. `find` must occur `count` times (only the first
 * is replaced), so a negative control whose text the module no longer holds fails rather than
 * building the module unchanged.
 */
function overlayPlugin(web, overlays) {
  const byPath = new Map();
  for (const { file, find, replace, count = 1 } of overlays) {
    const path = resolve(web, file);
    const text = byPath.get(path) ?? readFileSync(path, "utf8");
    const found = find === "" ? 0 : text.split(find).length - 1;
    if (found !== count) throw new Error(`the overlay patches ${JSON.stringify(find)}, which ${file} holds ${found} times, not ${count}`);
    byPath.set(path, text.replace(find, replace));
  }
  return {
    name: "overlay",
    setup(builder) {
      builder.onLoad({ filter: /.*/ }, (args) =>
        byPath.has(args.path) ? { contents: byPath.get(args.path), loader: args.path.endsWith(".ts") ? "ts" : "js" } : undefined);
    },
  };
}

/** The names a module exports, from an ESM build's metafile. */
async function exportedNames(esbuild, entry, plugins) {
  const result = await esbuild.build({
    entryPoints: [entry], bundle: true, format: "esm", write: false, metafile: true, logLevel: "silent",
    outfile: `${entry}.names.js`, plugins,
  });
  const [output] = Object.values(result.metafile.outputs);
  return [...output.exports].sort();
}

/**
 * The prelude the legacy script runs after (see above): `{ code, map, lineCount, names }`, or null
 * when web/src/legacy/imports.ts exports nothing. `minify` takes out white space and comments only,
 * as for the rest of the page; `overlays` patch module sources first (see overlayPlugin).
 */
export async function legacyPrelude(esbuild, web = WEB, { minify = true, overlays = [] } = {}) {
  const entry = join(web, LEGACY_IMPORTS);
  if (!existsSync(entry)) return null;
  const plugins = overlays.length > 0 ? [overlayPlugin(web, overlays)] : [];
  const names = await exportedNames(esbuild, entry, plugins);
  if (names.length === 0) return null;
  const result = await esbuild.build({
    entryPoints: [entry],
    bundle: true,
    format: "iife",
    globalName: PRELUDE_GLOBAL,
    platform: "neutral",
    write: false,
    outfile: join(web, "prelude.js"),
    sourcemap: "external",
    sourcesContent: true,
    target: "esnext",
    charset: "utf8",
    legalComments: "none",
    keepNames: true,
    minifyWhitespace: minify,
    logLevel: "silent",
    plugins,
  });
  if (result.warnings.length > 0) throw new Error(`the prelude: ${result.warnings.map((warning) => warning.text).join("; ")}`);
  const output = (suffix) => result.outputFiles.find((file) => file.path.endsWith(suffix));
  const bundled = output(".js").text;
  const body = bundled.endsWith("\n") ? bundled : `${bundled}\n`;
  const code = `const { ${names.join(", ")} } = (() => {\n"use strict";\n${body}return ${PRELUDE_GLOBAL};\n})();\n`;
  const map = JSON.parse(output(".js.map").text);
  // Two lines (the wrapper's opening) come before the bundle's first; its sources are named from the
  // repository root, like every other part's.
  map.mappings = `;;${map.mappings}`;
  map.sources = map.sources.map((source) => sourceName(source.split("\\").join("/")));
  return { name: LEGACY_IMPORTS, code, map, lineCount: code.split("\n").length - 1, names };
}

/**
 * The module at `entry` (relative to `web`) built for a test to import, with `overlays` patched into
 * its sources first: a negative control built exactly as the module is. Returns the module.
 */
export async function moduleForTests(entry, { web = WEB, overlays = [] } = {}) {
  const esbuild = loadTool("esbuild");
  const plugins = overlays.length > 0 ? [overlayPlugin(web, overlays)] : [];
  const result = await esbuild.build({
    entryPoints: [join(web, entry)], bundle: true, format: "esm", platform: "neutral", write: false,
    target: "esnext", logLevel: "silent", plugins,
  });
  const directory = mkdtempSync(join(tmpdir(), "vibe-talk-module-"));
  try {
    const file = join(directory, "module.mjs");
    writeFileSync(file, result.outputFiles[0].text);
    return await import(pathToFileURL(file).href);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

async function minifyAll(esbuild, web, names, loader) {
  const parts = [];
  for (const name of names) {
    const text = readFileSync(join(web, name), "utf8");
    if (loader === "js") requireStrict(name, text);
    const result = await esbuild.transform(text, {
      loader,
      minifyWhitespace: true,
      sourcemap: "external",
      sourcefile: sourceName(name),
      sourcesContent: true,
      target: "esnext",
      charset: "utf8",
      legalComments: "none",
      logLevel: "silent",
    });
    if (result.warnings.length > 0) {
      throw new Error(`${name}: ${result.warnings.map((warning) => warning.text).join("; ")}`);
    }
    const code = result.code.endsWith("\n") ? result.code : `${result.code}\n`;
    parts.push({ name, code, lineCount: code.split("\n").length - 1, map: JSON.parse(result.map) });
  }
  return parts;
}

/**
 * One asset from its minified parts: the joined bytes with a trailing `sourceMappingURL`, and its
 * map, each under its content hash. Returns `{ file, map, bytes, mapBytes }`.
 */
function assemble(parts, { stem, extension, leading, comment }) {
  const leadingLines = leading.split("\n").length - 1;
  const map = joinMaps(parts, leadingLines);
  const mapBytes = Buffer.from(JSON.stringify(map));
  const mapFile = `${stem}.${digest(mapBytes)}.${extension}.map`;
  const bytes = Buffer.from(leading + parts.map((part) => part.code).join("") + comment(mapFile));
  return { file: `${stem}.${digest(bytes)}.${extension}`, bytes, mapFile, mapBytes };
}

/**
 * The shell with its script tags replaced by one naming `scriptPath`, and its stylesheet links by
 * one naming `stylePath`. Every tag replaced must be one of the plain tags the build bundled, on a
 * line of its own, and afterwards the shell must load exactly the bundle and the sheet: a tag with
 * more to it (`defer`, `media=`), or one inside a comment, is refused rather than mis-linked.
 */
export function linkShell(html, scriptPath, stylePath) {
  const scripts = shellScripts(html);
  const styles = shellStyles(html);
  const out = linkTags(html, scriptPath, stylePath);
  const linkedScripts = shellScripts(out);
  const linkedStyles = shellStyles(out);
  const wanted = (list, path) => list.length === 1 && `/${list[0]}` === path;
  const plain = [...html.matchAll(/^[ \t]*<script src="\/([^"]+)"><\/script>\n/gm)].map((m) => m[1]);
  const plainStyles = [...html.matchAll(/^[ \t]*<link rel="stylesheet" href="\/([^"]+)" \/>\n/gm)].map((m) => m[1]);
  if (JSON.stringify(plain) !== JSON.stringify(scripts) || JSON.stringify(plainStyles) !== JSON.stringify(styles) ||
      !wanted(linkedScripts, scriptPath) || !wanted(linkedStyles, stylePath)) {
    throw new Error(
      `the shell's asset tags are not the plain, one-per-line tags the build can replace: it loads ` +
        `${JSON.stringify(scripts)} and ${JSON.stringify(styles)}, and would load ` +
        `${JSON.stringify(linkedScripts)} and ${JSON.stringify(linkedStyles)} once linked`
    );
  }
  return out;
}

function linkTags(html, scriptPath, stylePath) {
  const replaceRun = (text, pattern, tag) => {
    let first = true;
    let count = 0;
    const out = text.replace(pattern, (whole, indent) => {
      count += 1;
      if (!first) return "";
      first = false;
      return `${indent}${tag}`;
    });
    if (count === 0) throw new Error(`the shell has no ${pattern} to replace`);
    return out;
  };
  // Each tag on a line of its own, as the shell writes them: the first becomes the bundle's tag and
  // the rest go, line and all.
  let out = replaceRun(html, /^([ \t]*)<script src="\/[^"]+"><\/script>\n/gm, `<script src="${scriptPath}"></script>\n`);
  out = replaceRun(out, /^([ \t]*)<link rel="stylesheet" href="\/[^"]+" \/>\n/gm, `<link rel="stylesheet" href="${stylePath}" />\n`);
  return out;
}

/** Every file below `root`, as a `/`-separated path relative to it, sorted. */
export function filesBelow(root, prefix = "") {
  return readdirSync(join(root, prefix), { withFileTypes: true })
    .flatMap((entry) => {
      const path = prefix ? `${prefix}/${entry.name}` : entry.name;
      const directory = entry.isDirectory() || (entry.isSymbolicLink() && statSync(join(root, path)).isDirectory());
      return directory ? filesBelow(root, path) : [path];
    })
    .sort();
}

function writeTree(root, files) {
  for (const [path, bytes] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, bytes);
  }
}

/**
 * Refuse an output directory that is, or holds, the sources or the repository: publishing replaces
 * the whole directory, so `--out web` would replace the page's sources with their build.
 */
export function requireOutside(out, web = WEB) {
  const target = resolve(out);
  for (const tree of [web, WEB, TOOLS, REPOSITORY]) {
    const from = relative(target, tree);
    if (from === "" || (!from.startsWith("..") && !isAbsolute(from))) {
      throw new Error(`--out ${out} is or holds ${tree}, which the build would replace`);
    }
  }
  for (const sources of new Set([resolve(web), WEB])) {
    const inside = relative(sources, target);
    if (!inside.startsWith("..") && !isAbsolute(inside)) {
      throw new Error(`--out ${out} is inside ${sources}, so the build would become an input of the next one`);
    }
  }
}

/** Build into `out`, replacing whatever was there only once everything is written and checked. */
/**
 * Build the page whose sources are in `web` (by default this directory, vibe-talk/web) into `out`.
 * Another `web` is a copy of the sources with something changed on purpose: a browser walk's
 * negative control, built exactly as the page is. Its inputs are recorded under the same
 * `vibe-talk/web/` names, with the copy's digests.
 */
export async function build(out, { web = WEB } = {}) {
  requireOutside(out, web);
  const esbuild = loadTool("esbuild");
  const html = readFileSync(join(web, SHELL), "utf8");
  const scripts = shellScripts(html);
  const styles = shellStyles(html);
  const scriptParts = await minifyAll(esbuild, web, scripts, "js");
  const prelude = await legacyPrelude(esbuild, web);
  if (prelude) {
    const at = scriptParts.findIndex((part) => part.name === LEGACY_SCRIPT);
    if (at < 0) throw new Error(`the modules have a prelude for ${LEGACY_SCRIPT}, which the shell does not load`);
    scriptParts.splice(at, 0, prelude);
  }
  const js = assemble(scriptParts, {
    stem: "voice",
    extension: "js",
    leading: '"use strict";\n',
    comment: (mapFile) => `//# sourceMappingURL=/assets/${mapFile}\n`,
  });
  const css = assemble(await minifyAll(esbuild, web, styles, "css"), {
    stem: "voice",
    extension: "css",
    leading: "",
    comment: (mapFile) => `/*# sourceMappingURL=/assets/${mapFile} */\n`,
  });
  const shell = linkShell(html, `/assets/${js.file}`, `/assets/${css.file}`);

  const files = {
    [`assets/${js.file}`]: js.bytes,
    [`assets/${js.mapFile}`]: js.mapBytes,
    [`assets/${css.file}`]: css.bytes,
    [`assets/${css.mapFile}`]: css.mapBytes,
    [SHELL]: shell,
    "voice.js": js.bytes,
  };
  for (const name of ["contract.js", "style.css", "voice.css", "manifest.webmanifest"]) {
    files[name] = readFileSync(join(web, name));
  }
  const hashed = Object.keys(files).filter((path) => path.startsWith("assets/"));
  if (new Set(hashed.map((path) => basename(path))).size !== hashed.length) throw new Error("two assets share a name");

  // EVERY file below web/, and the lockfile: a prebuilt page (`VIBE_TALK_WEB_DIST`) is taken only
  // if this set matches the sources it is built into, so a file this build reads can never be left
  // out of the comparison, and one added since is a difference too.
  const inputs = {};
  for (const name of filesBelow(web)) inputs[sourceName(name)] = digest(readFileSync(join(web, name)));
  inputs["vibe-talk/contract/package-lock.json"] = digest(readFileSync(join(TOOLS, "package-lock.json")));
  const manifest = {
    page_build: digest(js.bytes),
    legacy_imports: prelude ? prelude.names : [],
    assets: {
      script: { path: `/assets/${js.file}`, map: `/assets/${js.mapFile}`, type: "text/javascript; charset=utf-8", sources: scripts },
      style: { path: `/assets/${css.file}`, map: `/assets/${css.mapFile}`, type: "text/css; charset=utf-8", sources: styles },
    },
    files: Object.fromEntries(Object.entries(files).map(([path, bytes]) => [path, digest(bytes)]).sort()),
    inputs,
    tools: { esbuild: esbuild.version },
  };
  files["assets.json"] = `${JSON.stringify(manifest, null, 2)}\n`;

  const staging = `${out}.tmp-${process.pid}-${randomBytes(4).toString("hex")}`;
  try {
    writeTree(staging, files);
    cpSync(join(web, "icons"), join(staging, "icons"), { recursive: true, dereference: true });
    // Read back: what is published is what was hashed.
    for (const [path, want] of Object.entries(manifest.files)) {
      if (digest(readFileSync(join(staging, path))) !== want) throw new Error(`${path} was not written as built`);
    }
    const old = `${out}.old-${process.pid}-${randomBytes(4).toString("hex")}`;
    let moved = false;
    try {
      renameSync(out, old);
      moved = true;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    try {
      renameSync(staging, out);
    } catch (error) {
      // The previous build goes back where it was rather than being stranded beside it.
      if (moved) renameSync(old, out);
      throw error;
    }
    if (moved) rmSync(old, { recursive: true, force: true });
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
  return manifest;
}

function usage() {
  return `usage: node web/build.mjs --out DIR [--web SOURCES]

Build the page the server embeds into DIR: assets/voice.<hash>.js and .css with their source maps,
the shell naming them, the plain files an older shell asks for, the manifest and icons, and
assets.json. DIR is replaced whole, once everything is written and checked.

  --out DIR      where to write the page; refused if it is or holds the sources or the repository
  --web SOURCES  the page's sources (default: the directory this script is in, vibe-talk/web): a
                 copy with something changed on purpose, such as a browser walk's negative control

Needs the esbuild and TypeScript that vibe-talk/contract/package-lock.json pins:
npm ci --prefix vibe-talk/contract
`;
}

/** The value after `flag` in `args`, or `fallback` when `flag` is absent. */
function option(args, flag, fallback) {
  const at = args.indexOf(flag);
  if (at === -1) return fallback;
  if (!args[at + 1] || args[at + 1].startsWith("--")) throw new Error(`${flag} needs a value`);
  return args[at + 1];
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const known = new Set(["--out", "--web"]);
  if (args.includes("--help") || args.includes("-h")) {
    process.stdout.write(usage());
  } else if (!args.includes("--out") || args.length % 2 !== 0 ||
             args.some((arg, i) => i % 2 === 0 && !known.has(arg))) {
    process.stderr.write(usage());
    process.exit(2);
  } else {
    const manifest = await build(resolve(option(args, "--out")), { web: resolve(option(args, "--web", WEB)) });
    process.stdout.write(`page build ${manifest.page_build}: ${manifest.assets.script.path} ${manifest.assets.style.path}\n`);
  }
}

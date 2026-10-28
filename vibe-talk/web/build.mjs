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
// `sourceMappingURL` naming the map, and the script's name is the hash of THOSE bytes, so neither
// hash covers itself. `page_build` in assets.json is the script's hash.
//
// HOW THE SCRIPTS ARE MINIFIED. The page's scripts are still classic scripts sharing one global
// scope, and one of them (voice.js) is in strict mode. Each is minified on its own, with whitespace
// and syntax minification only: no identifier is renamed, so every top-level name stays the global
// it was, every function keeps its `name`, and esbuild adds no helper of its own to the global
// scope. They are joined in the shell's order under one leading "use strict": a script that is not
// itself strict throughout is refused rather than made strict by the join.
//
// COMPATIBILITY. A shell already in a browser's cache names the plain /contract.js, /voice.js,
// /style.css and /voice.css. /voice.js is the new complete script (the validators run twice, which
// sets the same global twice); /contract.js stays the standalone validators, because a cached old
// /voice.js still needs them; the two stylesheets stay as written, in their own order.

import { createHash, randomBytes } from "node:crypto";
import { cpSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { SHELL, WEB, readWeb, shellScripts, shellStyles } from "./page_sources.mjs";

/** The lockfile-pinned toolchain: contract/package.json, installed by `npm ci --prefix contract`. */
const TOOLS = join(WEB, "..", "contract");
const REPOSITORY = resolve(WEB, "..", "..");

/** The first 128 bits of the SHA-256 of `bytes`, in lowercase hex. */
export const digest = (bytes) => createHash("sha256").update(bytes).digest("hex").slice(0, 32);

function loadEsbuild() {
  const require = createRequire(join(TOOLS, "package.json"));
  try {
    return require("esbuild");
  } catch (error) {
    throw new Error(
      `the page build needs the pinned esbuild in ${relative(process.cwd(), TOOLS) || TOOLS}/node_modules; ` +
        `install it with: npm ci --prefix ${join(WEB, "..", "contract")}\n(${error.message})`
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

/** A script is joined under one "use strict" only if it was already strict throughout. */
export function requireStrict(name, text) {
  const code = text.replace(/^(?:\s*\/\/[^\n]*\n)*\s*/, "");
  const strictFile = /^"use strict";/.test(code);
  const strictWrapper = /^\(function \(\) \{\s*"use strict";[\s\S]*\}\)\(\);\s*$/.test(code);
  if (!strictFile && !strictWrapper) {
    throw new Error(`${name} is not strict throughout, so joining it under "use strict" would change it`);
  }
}

const sourceName = (name) => relative(REPOSITORY, join(WEB, name)).split("\\").join("/");

async function minifyAll(esbuild, names, loader) {
  const parts = [];
  for (const name of names) {
    const text = readWeb(name);
    if (loader === "js") requireStrict(name, text);
    const result = await esbuild.transform(text, {
      loader,
      minifyWhitespace: true,
      minifySyntax: true,
      ...(loader === "css" ? { minifyIdentifiers: true } : {}),
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

/** The shell with its script tags replaced by one naming `script`, and its stylesheets by `style`. */
export function linkShell(html, scriptPath, stylePath) {
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

function writeTree(root, files) {
  for (const [path, bytes] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, bytes);
  }
}

/** Build into `out`, replacing whatever was there only once everything is written and checked. */
export async function build(out) {
  const esbuild = loadEsbuild();
  const html = readWeb(SHELL);
  const scripts = shellScripts(html);
  const styles = shellStyles(html);
  const js = assemble(await minifyAll(esbuild, scripts, "js"), {
    stem: "voice",
    extension: "js",
    leading: '"use strict";\n',
    comment: (mapFile) => `//# sourceMappingURL=${mapFile}\n`,
  });
  const css = assemble(await minifyAll(esbuild, styles, "css"), {
    stem: "voice",
    extension: "css",
    leading: "",
    comment: (mapFile) => `/*# sourceMappingURL=${mapFile} */\n`,
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
    files[name] = readFileSync(join(WEB, name));
  }
  const hashed = Object.keys(files).filter((path) => path.startsWith("assets/"));
  if (new Set(hashed.map((path) => basename(path))).size !== hashed.length) throw new Error("two assets share a name");

  const inputs = {};
  for (const name of [SHELL, ...scripts, ...styles, "manifest.webmanifest", "page_sources.mjs", "build.mjs"]) {
    inputs[sourceName(name)] = digest(readFileSync(join(WEB, name)));
  }
  inputs["vibe-talk/contract/package-lock.json"] = digest(readFileSync(join(TOOLS, "package-lock.json")));
  const manifest = {
    page_build: digest(js.bytes),
    assets: {
      script: { path: `/assets/${js.file}`, map: `/assets/${js.mapFile}`, sources: scripts },
      style: { path: `/assets/${css.file}`, map: `/assets/${css.mapFile}`, sources: styles },
    },
    files: Object.fromEntries(Object.entries(files).map(([path, bytes]) => [path, digest(bytes)]).sort()),
    inputs,
    tools: { esbuild: esbuild.version },
  };
  files["assets.json"] = `${JSON.stringify(manifest, null, 2)}\n`;

  const staging = `${out}.tmp-${process.pid}-${randomBytes(4).toString("hex")}`;
  rmSync(staging, { recursive: true, force: true });
  try {
    writeTree(staging, files);
    cpSync(join(WEB, "icons"), join(staging, "icons"), { recursive: true, dereference: true });
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
    renameSync(staging, out);
    if (moved) rmSync(old, { recursive: true, force: true });
  } catch (error) {
    rmSync(staging, { recursive: true, force: true });
    throw error;
  }
  return manifest;
}

function usage() {
  return "usage: node web/build.mjs --out DIR\n";
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const at = args.indexOf("--out");
  if (args.includes("--help") || args.includes("-h")) {
    process.stdout.write(usage());
  } else if (at === -1 || !args[at + 1] || args.length !== 2) {
    process.stderr.write(usage());
    process.exit(2);
  } else {
    const manifest = await build(resolve(args[at + 1]));
    process.stdout.write(`page build ${manifest.page_build}: ${manifest.assets.script.path} ${manifest.assets.style.path}\n`);
  }
}


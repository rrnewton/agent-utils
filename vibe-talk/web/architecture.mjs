// The page's import directions, checked (`#624 voice-page-modularity`, plan §3 "enforced import
// boundaries"). Every module under web/src belongs to one package; a package may import only the
// packages its row below names, and another package only through that package's index.ts. Every
// import is a relative path to a file that exists, read by the pinned TypeScript parser, so a
// type-only import, a re-export and an alias are edges like any other; dynamic imports, require
// and bare package names are refused outright, and so is a cycle between files. It fails closed:
// a module in a directory no row names is itself a violation, so a new package cannot escape by
// being new.
//
// It holds what the modules say as well as what they import (plan §3, "type-check ownership as
// well as imports"). In every handwritten module: no `any`, no comment that turns the checker off
// (@ts-nocheck, @ts-ignore, @ts-expect-error), and no exported binding another module could see
// change. And outside the browser's implementation of the ports and the legacy script, none of the
// browser's own capabilities (`PLATFORM_GLOBALS`): the network, storage and the audio hardware
// reach a module through its ports. The core is held to that by its library as well
// (tsconfig.core.json); views have the DOM, and are held to it here.
//
//   node web/architecture.mjs [--src DIR]     prints every violation; exits 1 if there is one
//
// Run by `make -C vibe-talk typecheck` and by tests/js/page_build.test.mjs.

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { WEB } from "./page_sources.mjs";
import { loadTool } from "./build.mjs";

/** The portable foundations every layer above may use. */
const BASE = ["contract", "foundation", "ports"];

/**
 * Each package, and the packages it may import. The plan's direction, bottom up: the contract,
 * foundations and ports import no layer above them; core services take only those (a core
 * package's use of another core package is named here when it is wanted, never by default);
 * the browser's implementation of the ports takes the same; the legacy script's imports are the
 * one composition point, and take anything until the legacy script is gone.
 */
export const PACKAGES = Object.freeze({
  contract: [],
  foundation: [],
  ports: [],
  "core/annotations": BASE,
  "core/api": BASE,
  "core/call": BASE,
  "core/channels": BASE,
  "core/navigation": BASE,
  "core/outbox": BASE,
  "core/search": BASE,
  "core/settings": BASE,
  "core/speech": BASE,
  "core/sync": BASE,
  "platform/web": BASE,
  "view/composer": [...BASE, "view/dom"],
  "view/dom": BASE,
  legacy: "*",
});

/**
 * The browser capabilities that are web/src/platform/web's to wrap, as the ports declare them: what no
 * other package names, as a global or as a property of `window`, `self` or `globalThis`. A view
 * that needs one takes the port, as the core does.
 */
export const PLATFORM_GLOBALS = Object.freeze([
  "fetch", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB",
  "caches", "navigator", "AudioContext", "webkitAudioContext", "Audio", "MediaRecorder", "speechSynthesis",
  "SpeechSynthesisUtterance", "Notification",
]);
/** The packages that may name `PLATFORM_GLOBALS`. */
const PLATFORM_PACKAGES = new Set(["platform/web", "legacy"]);
/** The objects whose properties are globals. */
const GLOBAL_OBJECTS = new Set(["window", "self", "globalThis"]);

/**
 * What one handwritten module says that the page's modules do not: `{ line, what }` for each `any`,
 * each comment turning the checker off, each exported binding that can change, and, unless the
 * module is allowed them, each use of a `PLATFORM_GLOBALS` capability.
 */
function sayingsOf(ts, file, text, platformAllowed) {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true,
    file.endsWith(".ts") ? ts.ScriptKind.TS : ts.ScriptKind.JS);
  const found = [];
  const at = (pos) => source.getLineAndCharacterOfPosition(pos).line + 1;
  // Every comment, wherever it sits: the scanner, not a pattern, so a directive inside a string is not one.
  const scanner = ts.createScanner(ts.ScriptTarget.Latest, false, source.languageVariant, text);
  for (let kind = scanner.scan(); kind !== ts.SyntaxKind.EndOfFileToken; kind = scanner.scan()) {
    if ((kind === ts.SyntaxKind.SingleLineCommentTrivia || kind === ts.SyntaxKind.MultiLineCommentTrivia) &&
        /@ts-(?:nocheck|ignore|expect-error)\b/.test(scanner.getTokenText())) {
      found.push({ line: at(scanner.getTokenStart()), what: `a comment that turns type checking off (${scanner.getTokenText().trim().slice(0, 40)})` });
    }
  }
  const mutable = new Set();
  for (const statement of source.statements) {
    if (ts.isVariableStatement(statement) && !(statement.declarationList.flags & ts.NodeFlags.Const)) {
      for (const declaration of statement.declarationList.declarations) {
        if (ts.isIdentifier(declaration.name)) mutable.add(declaration.name.text);
      }
      if (statement.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword)) {
        found.push({ line: at(statement.getStart(source)), what: "an exported binding that can change (export let or var)" });
      }
    }
  }
  const visit = (node) => {
    if (!platformAllowed && ts.isElementAccessExpression(node) && ts.isIdentifier(node.expression) &&
        GLOBAL_OBJECTS.has(node.expression.text) && ts.isStringLiteralLike(node.argumentExpression) &&
        PLATFORM_GLOBALS.includes(node.argumentExpression.text)) {
      found.push({ line: at(node.getStart(source)), what: `the browser's ${node.argumentExpression.text}, which reaches a module through its port (web/src/ports)` });
    }
    if (node.kind === ts.SyntaxKind.AnyKeyword) {
      found.push({ line: at(node.getStart(source)), what: "the type `any`" });
    } else if (ts.isExportDeclaration(node) && !node.moduleSpecifier && node.exportClause && ts.isNamedExports(node.exportClause)) {
      for (const element of node.exportClause.elements) {
        const local = (element.propertyName ?? element.name).text;
        if (mutable.has(local)) found.push({ line: at(element.getStart(source)), what: `an exported binding that can change (${local} is a let or var)` });
      }
    } else if (!platformAllowed && ts.isIdentifier(node) && PLATFORM_GLOBALS.includes(node.text)) {
      const parent = node.parent;
      const named = ts.isPropertyAccessExpression(parent) && parent.name === node;
      const onGlobal = named && ts.isIdentifier(parent.expression) && GLOBAL_OBJECTS.has(parent.expression.text);
      const declared = (ts.isPropertySignature(parent) || ts.isPropertyDeclaration(parent) || ts.isMethodSignature(parent) ||
        ts.isMethodDeclaration(parent) || ts.isPropertyAssignment(parent) || ts.isBindingElement(parent) ||
        ts.isParameter(parent) || ts.isVariableDeclaration(parent) || ts.isImportSpecifier(parent) ||
        ts.isExportSpecifier(parent)) && parent.name === node;
      // A type of that name is no use of the capability: `typeof fetch` or a parameter typed `Audio`.
      const typeOnly = ts.isTypeReferenceNode(parent) || ts.isTypeQueryNode(parent) || ts.isQualifiedName(parent);
      if ((!named && !declared && !typeOnly) || onGlobal) {
        found.push({ line: at(node.getStart(source)), what: `the browser's ${node.text}, which reaches a module through its port (web/src/ports)` });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

/** The package `file` (relative to src, with "/") belongs to: the longest row that contains it, or null. */
function packageOf(file) {
  let best = null;
  for (const name of Object.keys(PACKAGES)) {
    if ((file === name || file.startsWith(`${name}/`)) && (best === null || name.length > best.length)) best = name;
  }
  return best;
}

/** Every module file below `dir`, relative to it, with "/". */
function modulesBelow(dir, root = dir) {
  const out = [];
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...modulesBelow(path, root));
    else if (/\.(ts|js|mjs)$/.test(name)) out.push(relative(root, path).split(sep).join("/"));
  }
  return out;
}

/**
 * Every import edge in one module: `{ specifier, line, kind }`, where kind is "static" (an import,
 * an export ... from, an import type, an `import("x")` type) or a refused form ("dynamic",
 * "require", "import-equals").
 */
function importsOf(ts, file, text) {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true,
    file.endsWith(".ts") ? ts.ScriptKind.TS : ts.ScriptKind.JS);
  const edges = [];
  const at = (node) => source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
  const visit = (node) => {
    if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier &&
        ts.isStringLiteral(node.moduleSpecifier)) {
      edges.push({ specifier: node.moduleSpecifier.text, line: at(node), kind: "static" });
    } else if (ts.isImportEqualsDeclaration(node)) {
      edges.push({ specifier: node.moduleReference.getText(source), line: at(node), kind: "import-equals" });
    } else if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument) && ts.isStringLiteral(node.argument.literal)) {
      edges.push({ specifier: node.argument.literal.text, line: at(node), kind: "static" });
    } else if (ts.isCallExpression(node)) {
      if (node.expression.kind === ts.SyntaxKind.ImportKeyword) {
        edges.push({ specifier: node.arguments[0] ? node.arguments[0].getText(source) : "", line: at(node), kind: "dynamic" });
      } else if (ts.isIdentifier(node.expression) && node.expression.text === "require") {
        edges.push({ specifier: node.arguments[0] ? node.arguments[0].getText(source) : "", line: at(node), kind: "require" });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return edges;
}

/**
 * Every violation of the page's import directions under `src` (default web/src), as sentences
 * naming the file and line; empty when there are none.
 */
export function checkArchitecture(src = join(WEB, "src"), ts = loadTool("typescript")) {
  const problems = [];
  const files = modulesBelow(src);
  /** @type {Map<string, string[]>} */
  const graph = new Map();
  for (const file of files) {
    const own = packageOf(file);
    if (own === null) {
      problems.push(`${file}: in no package of web/architecture.mjs PACKAGES; add its directory there, with what it may import`);
    }
    const next = [];
    graph.set(file, next);
    const text = readFileSync(join(src, file), "utf8");
    if (!text.startsWith("// GENERATED by ")) {
      for (const { line, what } of sayingsOf(ts, file, text, own !== null && PLATFORM_PACKAGES.has(own))) {
        problems.push(`${file}:${line}: ${what}`);
      }
    }
    for (const { specifier, line, kind } of importsOf(ts, file, text)) {
      const where = `${file}:${line}`;
      if (kind !== "static") {
        problems.push(`${where}: a ${kind} import (${specifier}); the page's modules import statically, by relative path`);
        continue;
      }
      if (!specifier.startsWith("./") && !specifier.startsWith("../")) {
        problems.push(`${where}: imports "${specifier}", which is not a relative path; the page's modules use no packages`);
        continue;
      }
      const target = relative(src, resolve(dirname(join(src, file)), specifier)).split(sep).join("/");
      if (target.startsWith("..") || !existsSync(join(src, target)) || statSync(join(src, target)).isDirectory()) {
        problems.push(`${where}: imports "${specifier}", which is no module under web/src`);
        continue;
      }
      next.push(target);
      const theirs = packageOf(target);
      if (own === null || theirs === null || theirs === own) continue;
      const allowed = PACKAGES[own];
      if (allowed !== "*" && !allowed.includes(theirs)) {
        problems.push(`${where}: ${own} may not import ${theirs} ("${specifier}"); ${own} may import ${allowed.length ? allowed.join(", ") : "no other package"}`);
      }
      if (target !== `${theirs}/index.ts`) {
        problems.push(`${where}: imports ${target}, inside ${theirs}; another package is imported only through ${theirs}/index.ts`);
      }
    }
  }
  problems.push(...cycles(graph));
  return problems;
}

/** Every cycle between files in `graph`, once each, as a sentence. */
function cycles(graph) {
  const found = [];
  const state = new Map();
  const stack = [];
  const visit = (file) => {
    state.set(file, "open");
    stack.push(file);
    for (const next of graph.get(file) || []) {
      if (state.get(next) === "open") {
        found.push(`a cycle between files: ${[...stack.slice(stack.indexOf(next)), next].join(" -> ")}`);
      } else if (!state.has(next)) {
        visit(next);
      }
    }
    stack.pop();
    state.set(file, "done");
  };
  for (const file of graph.keys()) if (!state.has(file)) visit(file);
  return found;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const at = process.argv.indexOf("--src");
  const problems = checkArchitecture(at > 0 ? resolve(process.argv[at + 1]) : undefined);
  for (const problem of problems) console.error(problem);
  if (problems.length > 0) process.exit(1);
  console.log("web/src: every import goes the page's way, and no module says what the page's modules do not");
}

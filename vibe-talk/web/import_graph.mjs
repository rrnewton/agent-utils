#!/usr/bin/env node
// Static import graph for vibe-talk/web/src (#2059 vibe-talk-ui-latency-architecture-review).
// Prints a Markdown dependency map: per-package size, package-to-package edges,
// layer rollup, direction violations against architecture.mjs PACKAGES, and file cycles.
// Usage: node vibe-talk/web/import_graph.mjs [--src DIR] [--json]
// No dependencies beyond Node builtins; specifiers are read with a regex, so
// dynamic import() targets are counted as edges but never resolved by execution.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PACKAGES } from "./architecture.mjs";

const WEB = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const srcArg = args.indexOf("--src");
const SRC = srcArg >= 0 ? resolve(args[srcArg + 1]) : join(WEB, "src");
const asJson = args.includes("--json");

// Brief layers (#2059): view, state, core/api, voice/speech, server sync.
// present/* is DOM-free presentation, so it rolls up with view except
// present/call, which presents the voice call and rolls up with voice/speech.
export const LAYERS = Object.freeze({
  view: ["view/channel", "view/composer", "view/catchup", "view/documents",
    "view/transcript", "view/shell", "view/settings", "view/dom",
    "present/rows", "present/notices", "present/settings", "present/time"],
  state: ["core/channels", "core/outbox", "core/annotations", "core/navigation",
    "core/settings", "core/search", "core/transcripts"],
  "core/api": ["core/api", "contract"],
  "voice/speech": ["core/speech", "core/call", "view/voice", "present/call"],
  "server sync": ["core/sync"],
  foundation: ["foundation", "ports", "platform/web"],
  app: ["app"],
});
const layerOf = new Map();
for (const [layer, pkgs] of Object.entries(LAYERS)) {
  for (const p of pkgs) layerOf.set(p, layer);
}

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else if (/\.(ts|js|mjs)$/.test(name)) out.push(path);
  }
  return out;
}

export function packageOf(relPath) {
  const parts = relPath.split("/");
  if (parts.length < 2) return parts[0];
  if (["core", "present", "view", "platform"].includes(parts[0])) {
    return `${parts[0]}/${parts[1]}`;
  }
  return parts[0];
}

const SPECIFIER = /(?:import|export)\s[^"']*?from\s*["']([^"']+)["']|import\s*["']([^"']+)["']|import\s*\(\s*["']([^"']+)["']\s*\)/g;
const JSDOC_SPECIFIER = /import\s*\(\s*["']([^"']+)["']\s*\)/g;

// Split code from comments: JSDoc import("...") type refs live in comments and
// are not import edges for architecture.mjs (its TS walk never enters them in
// .ts files), so they are reported separately instead of graphed.
export function splitComments(text) {
  const comments = [];
  const code = text.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, (m) => {
    comments.push(m);
    return " ".repeat(m.length);
  });
  return { code, comments: comments.join("\n") };
}

export function specifiersOf(text) {
  const { code, comments } = splitComments(text);
  const edges = [];
  for (const m of code.matchAll(SPECIFIER)) edges.push(m[1] ?? m[2] ?? m[3]);
  const jsdoc = [];
  for (const m of comments.matchAll(JSDOC_SPECIFIER)) jsdoc.push(m[1]);
  return { edges, jsdoc };
}

function resolveSpecifier(fromFile, spec, files, src) {
  if (!spec.startsWith(".")) return null; // bare specifier: external, no edge
  // JSDoc type refs in this tree are written web-root-relative ("./src/...").
  const base = spec.startsWith("./src/")
    ? resolve(src, spec.slice("./src/".length))
    : resolve(dirname(fromFile), spec);
  for (const cand of [base, `${base}.ts`, `${base}.js`,
    join(base, "index.ts"), join(base, "index.js"),
    base.replace(/\.js$/, ".ts")]) {
    if (files.has(cand)) return cand;
  }
  return "unresolved";
}

export function buildGraph(src = SRC) {
  const paths = walk(src).sort();
  const files = new Set(paths);
  const nodes = new Map(); // rel -> {lines, pkg, edges:Set<rel>, unresolved:[]}
  for (const path of paths) {
    const rel = relative(src, path);
    const text = readFileSync(path, "utf8");
    const edges = new Set();
    const unresolved = [];
    const found = specifiersOf(text);
    for (const spec of found.edges) {
      const target = resolveSpecifier(path, spec, files, src);
      if (target === "unresolved") unresolved.push(spec);
      else if (target) edges.add(relative(src, target));
    }
    nodes.set(rel, {
      lines: text ? text.split("\n").length - (text.endsWith("\n") ? 1 : 0) : 0,
      pkg: packageOf(rel), edges, unresolved, jsdoc: found.jsdoc,
    });
  }
  return nodes;
}

function cycles(nodes) {
  // Tarjan SCC; any SCC with >1 member (or a self-edge) is a cycle.
  let index = 0;
  const idx = new Map(), low = new Map(), onStack = new Set(), stack = [], out = [];
  const visit = (v) => {
    idx.set(v, index); low.set(v, index); index++;
    stack.push(v); onStack.add(v);
    for (const w of nodes.get(v).edges) {
      if (!idx.has(w)) { visit(w); low.set(v, Math.min(low.get(v), low.get(w))); }
      else if (onStack.has(w)) low.set(v, Math.min(low.get(v), idx.get(w)));
    }
    if (low.get(v) === idx.get(v)) {
      const scc = [];
      let w;
      do { w = stack.pop(); onStack.delete(w); scc.push(w); } while (w !== v);
      if (scc.length > 1 || nodes.get(v).edges.has(v)) out.push(scc.sort());
    }
  };
  for (const v of nodes.keys()) if (!idx.has(v)) visit(v);
  return out;
}

export function summarize(nodes) {
  const pkgs = new Map(); // pkg -> {files, lines, out:Map<pkg,Set<file>>}
  const fanIn = new Map(), fanOut = new Map();
  const violations = [], unresolvedAll = [], jsdocAll = [];
  for (const [rel, node] of nodes) {
    if (!pkgs.has(node.pkg)) pkgs.set(node.pkg, { files: 0, lines: 0, out: new Map() });
    const p = pkgs.get(node.pkg);
    p.files++; p.lines += node.lines;
    fanOut.set(rel, node.edges.size);
    for (const spec of node.unresolved) unresolvedAll.push(`${rel}: ${spec}`);
    for (const spec of node.jsdoc ?? []) jsdocAll.push(`${rel}: ${spec}`);
    for (const target of node.edges) {
      fanIn.set(target, (fanIn.get(target) ?? 0) + 1);
      const tp = nodes.get(target).pkg;
      if (tp === node.pkg) continue;
      if (!p.out.has(tp)) p.out.set(tp, new Set());
      p.out.get(tp).add(rel);
      const allowed = PACKAGES[node.pkg];
      if (!allowed) violations.push(`${rel}: unknown package ${node.pkg}`);
      else if (!allowed.includes(tp)) {
        violations.push(`${rel} -> ${target}: ${node.pkg} may not import ${tp}`);
      }
      if (!target.endsWith("index.ts") && !target.endsWith("index.js")) {
        violations.push(`${rel} -> ${target}: cross-package import bypasses index`);
      }
    }
  }
  const layerEdges = new Map(); // "from -> to" -> importing-file count
  for (const [pkg, p] of pkgs) {
    for (const [tp, importers] of p.out) {
      const key = `${layerOf.get(pkg) ?? "?"} -> ${layerOf.get(tp) ?? "?"}`;
      layerEdges.set(key, (layerEdges.get(key) ?? 0) + importers.size);
    }
  }
  const top = (map, n) => [...map.entries()].sort((a, b) => b[1] - a[1]).slice(0, n);
  return {
    files: nodes.size,
    lines: [...nodes.values()].reduce((n, x) => n + x.lines, 0),
    pkgs, layerEdges, violations, unresolvedAll, jsdocAll,
    cycles: cycles(nodes), topFanIn: top(fanIn, 12), topFanOut: top(fanOut, 12),
  };
}

function toMarkdown(nodes, s) {
  const out = [];
  out.push(`Files: ${s.files}  Lines: ${s.lines}  Cycles: ${s.cycles.length}  ` +
    `Direction/index violations: ${s.violations.length}  Unresolved specifiers: ${s.unresolvedAll.length}  ` +
    `JSDoc-only type refs (not graphed): ${s.jsdocAll.length}`, "");
  out.push("| Package | Layer | Files | Lines | Imports (package: importing files) |", "|---|---|--:|--:|---|");
  for (const pkg of [...s.pkgs.keys()].sort()) {
    const p = s.pkgs.get(pkg);
    const imports = [...p.out.entries()].sort()
      .map(([tp, set]) => `${tp} (${set.size})`).join(", ") || "—";
    out.push(`| ${pkg} | ${layerOf.get(pkg) ?? "?"} | ${p.files} | ${p.lines} | ${imports} |`);
  }
  out.push("", "| Layer edge | Importing files |", "|---|--:|");
  for (const [key, n] of [...s.layerEdges.entries()].sort()) out.push(`| ${key} | ${n} |`);
  out.push("", "Top fan-in (most imported files):",
    ...s.topFanIn.map(([f, n]) => `- ${f}: ${n}`),
    "", "Top fan-out (files importing the most modules):",
    ...s.topFanOut.map(([f, n]) => `- ${f}: ${n}`));
  if (s.cycles.length) out.push("", "Cycles:", ...s.cycles.map((c) => `- ${c.join(" <-> ")}`));
  if (s.violations.length) out.push("", "Violations:", ...s.violations.map((v) => `- ${v}`));
  if (s.unresolvedAll.length) out.push("", "Unresolved:", ...s.unresolvedAll.map((v) => `- ${v}`));
  if (s.jsdocAll.length) out.push("", "JSDoc-only type refs (comments, not import edges):",
    ...s.jsdocAll.map((v) => `- ${v}`));
  return out.join("\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (!existsSync(SRC)) { console.error(`no such src dir: ${SRC}`); process.exit(2); }
  const nodes = buildGraph(SRC);
  const s = summarize(nodes);
  if (asJson) {
    console.log(JSON.stringify({
      files: s.files, lines: s.lines, cycles: s.cycles,
      violations: s.violations, unresolved: s.unresolvedAll, jsdocOnly: s.jsdocAll,
      packages: Object.fromEntries([...s.pkgs].map(([k, v]) =>
        [k, { files: v.files, lines: v.lines, layer: layerOf.get(k) ?? null,
          imports: Object.fromEntries([...v.out].map(([t, set]) => [t, set.size])) }])),
      layerEdges: Object.fromEntries(s.layerEdges),
      topFanIn: s.topFanIn, topFanOut: s.topFanOut,
    }, null, 2));
  } else {
    console.log(toMarkdown(nodes, s));
  }
  process.exit(s.violations.length || s.cycles.length ? 1 : 0);
}

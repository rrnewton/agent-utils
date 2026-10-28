// The page's source files, and the order its shell loads them in.
//
// `#624 voice-page-modularity`. More than one program needs the page as the browser gets it: the
// fake-DOM suite runs its scripts, and the build the refactor introduces bundles them. Each used to
// name `voice.js` and `contract.js` itself, so a change to what the shell loads had to be made in
// every one of them. The shell, `voice.html`, already states which scripts and stylesheets run and
// in what order; this reads that, so there is one list and it is the one the browser follows.
//
// Not served: the server's static table (src/http/assets.rs) names every file it serves, and this
// is not among them.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** The directory holding the page's sources: vibe-talk/web. */
export const WEB = dirname(fileURLToPath(import.meta.url));

/** The page's HTML shell, the app served at `/`. */
export const SHELL = "voice.html";

/** The text of the page file `name`, relative to `WEB`. */
export const readWeb = (name) => readFileSync(join(WEB, name), "utf8");

/** `html` without its comments, which name files they do not load. */
const markup = (html) => html.replace(/<!--[\s\S]*?-->/g, " ");

/**
 * The scripts `html` loads, as the file names below `WEB` its `<script src="/...">` tags give, in
 * the order the browser runs them. Every script tag must name a local file: an inline or
 * off-origin script is not something this page has, and a reader of this list would miss it.
 */
export function shellScripts(html) {
  return [...markup(html).matchAll(/<script\b([^>]*)>/g)].map(([tag, attributes]) => {
    const src = /\bsrc="\/(?!\/)([^"?#]+)[^"]*"/.exec(attributes);
    if (!src) throw new Error(`the shell has a script that names no local file: ${tag}`);
    return src[1];
  });
}

/** The stylesheets `html` links, as file names below `WEB`, in the order they cascade. */
export function shellStyles(html) {
  return [...markup(html).matchAll(/<link\b([^>]*)>/g)]
    .filter(([, attributes]) => /\brel="stylesheet"/.test(attributes))
    .map(([tag, attributes]) => {
      const href = /\bhref="\/(?!\/)([^"?#]+)[^"]*"/.exec(attributes);
      if (!href) throw new Error(`the shell links a stylesheet that names no local file: ${tag}`);
      return href[1];
    });
}

/** How many times `needle` occurs in `text`, overlapping occurrences not counted twice. */
export const occurrences = (text, needle) => (needle === "" ? 0 : text.split(needle).length - 1);

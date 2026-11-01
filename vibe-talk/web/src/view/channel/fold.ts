// What a folded row's body holds, as the fold model reads it, and the line under it that says how much
// is hidden (`#624 voice-page-modularity`, moved from the legacy script into view/channel; `#244
// elision-word-count`). The DOM half of the fold: present/rows counts, this reads and draws.

import { elisionLabels } from "../../present/rows/index.ts";

/** The class on the words a heading restates. The server writes it too: `RESTATED_CLASS` in src/render.rs. */
const RESTATED_CLASS = "rh-src";

/** Elements whose words flow on the line they are on: their text is the line's. */
const FOLD_INLINE = new Set(["a", "code", "del", "em", "input", "s", "span", "strong"]);

/** Elements that start a line and end one, whose own text flows in lines the clamp counts. */
const FOLD_FLOW_BLOCKS = new Set(["blockquote", "div", "h1", "h2", "h3", "h4", "hr", "li", "ol", "p", "ul"]);

/** The text of `node`'s text nodes, in order, joined with `between`: what a code block holds, as written. */
function textNodesOf(node: Node, between = ""): string {
  const parts: string[] = [];
  const walk = (at: Node): void => {
    for (const kid of at.childNodes) {
      if (kid.nodeType === 3) parts.push(kid.textContent as string);
      else if (kid.nodeType === 1) walk(kid);
    }
  };
  walk(node);
  return parts.join(between);
}

/**
 * What a drawn body holds, as the fold's model reads it: RUNS, `[kind, text]`, in order. `t` is text
 * that flows, "\n" in it a line end the browser draws (a `<br>`, the edge of a block), any other
 * white space a space; `m` the same in code's face (inline code); `p` one code block's text, a line
 * of the block for each of its lines; `w` the words of a block drawn WHOLE, whose lines the clamp
 * does not count (a table). Null when the body holds an element there is no rule for here. With
 * `skipClass`, the elements of exactly that class are left out, as `renderedText` leaves them.
 */
function foldRuns(node: Node, skipClass = ""): Array<[string, string]> | null {
  const runs: Array<[string, string]> = [];
  const add = (kind: string, text: string): void => {
    const last = runs[runs.length - 1];
    if (last && last[0] === kind) last[1] += text;
    else runs.push([kind, text]);
  };
  let known = true;
  const walk = (at: Node, mono: boolean): void => {
    for (const kid of at.childNodes) {
      if (!known) return;
      if (kid.nodeType === 3) {
        add(mono ? "m" : "t", String(kid.textContent).replace(/\s/g, " "));
        continue;
      }
      if (kid.nodeType !== 1) continue;
      if (skipClass && (kid as Element).className === skipClass) continue;
      const tag = String((kid as Element).tagName).toLowerCase();
      if (tag === "br") {
        add(mono ? "m" : "t", "\n");
      } else if (FOLD_INLINE.has(tag)) {
        walk(kid, mono || tag === "code");
      } else if (FOLD_FLOW_BLOCKS.has(tag)) {
        add("t", "\n");
        walk(kid, mono);
        add("t", "\n");
      } else if (tag === "pre") {
        runs.push(["p", textNodesOf(kid)]);
      } else if (tag === "table") {
        // Every word of it, a space between one cell's and the next.
        runs.push(["w", textNodesOf(kid, " ")]);
      } else {
        known = false;
      }
    }
  };
  walk(node, false);
  return known ? runs : null;
}

/**
 * The line under a folded message: one span per label, each saying the width classes it is for
 * (`data-w`, indexes into `FOLD_WIDTHS_EM`) and, on a rich header's row, whether it counts the text
 * with the header showing or without (`data-rh`). web/voice.css shows the one that applies. Null when
 * no width has anything to say.
 * `versions` is what the row can show, as `elisionLabels` takes it, counted against `collapsedLines`.
 */
function elisionLine(versions: ReadonlyArray<{ runs: Array<[string, string]> | string | null; rh: string | null }>, collapsedLines: number): HTMLElement | null {
  const spans = new Map<string, { label: string; rh: string | null; at: number[] }>();
  for (const { runs, rh } of versions) {
    elisionLabels(runs, collapsedLines).forEach((label, at) => {
      if (!label) return;
      const key = `${rh || ""}|${label}`;
      if (!spans.has(key)) spans.set(key, { label, rh, at: [] });
      (spans.get(key) as { at: number[] }).at.push(at);
    });
  }
  if (spans.size === 0) return null;
  const line = document.createElement("div");
  line.className = "elided";
  for (const { label, rh, at } of spans.values()) {
    const span = document.createElement("span");
    span.setAttribute("data-w", at.join(" "));
    if (rh) span.setAttribute("data-rh", rh);
    span.textContent = label;
    line.append(span);
  }
  return line;
}

/**
 * `#244 elision-word-count`. The line under a folded row's text (`elisionLine`), from what `body`
 * draws: a channel row's body is drawn (`.md` boxes), and is read for its blocks as well as its
 * words (`foldRuns`); a spoken turn's is its text. A rich header's row (`#242 rich-headers`) shows
 * its text without the words the header restates while the header shows. Null when no width has
 * anything to say. Counted against `collapsedLines`, the lines the fold shows.
 */
function foldedTextLine(body: Element, text: unknown, collapsedLines: number): HTMLElement | null {
  const first = body.children[0];
  const headed = Boolean(first && first.hasAttribute && first.hasAttribute("data-rh"));
  const drawn = (skipClass: string): Array<[string, string]> | string | null => (first ? foldRuns(body, skipClass) : String(text === null || text === undefined ? "" : text));
  return elisionLine(headed
    ? [{ runs: drawn(RESTATED_CLASS), rh: "on" }, { runs: drawn(""), rh: "off" }]
    : [{ runs: drawn(""), rh: null }], collapsedLines);
}

export { FOLD_FLOW_BLOCKS, FOLD_INLINE, RESTATED_CLASS, elisionLine, foldRuns, foldedTextLine, textNodesOf };

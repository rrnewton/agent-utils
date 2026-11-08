// One message's body as DOM (`#242 rich-headers`, `#129 message-search`; `#624
// voice-page-modularity`, moved from the legacy script into view/channel; mu-readcore):
// the server's rendering inserted as nodes (the page's sole template-innerHTML site), plain
// text as paragraphs, the heading's restated mark, and what a body links to. Stateless
// except the per-message text cache, which holds text only, never nodes.

import type { Message } from "../../contract/index.ts";
import { HTTP_ADDRESS, linksIn } from "../../foundation/index.ts";
import { RESTATED_CLASS } from "./fold.ts";
import { headingOf } from "../../present/rows/index.ts";
import type { MessageHeading } from "../../present/rows/index.ts";
import { rawTextOf, renderedHtmlOf } from "../../present/rows/index.ts";
import { renderedText } from "./text.ts";
import { keepCardUnits } from "./cardtext.ts";
import { cardMarkTarget } from "./cardtext.ts";
import { tagNameOf } from "../dom/index.ts";

/** What building bodies asks of the page. */
export interface MessageBodyDependencies {
  wireMessageLinks(node: HTMLElement): void;
}

/** The body builders: the legacy names, unchanged. */
export interface MessageBodies {
  insertSanitizedHtml(parent: HTMLElement, contentHtml: string): void;
  plainParagraphs(parent: HTMLElement, text: string, cut?: number): void;
  messageBody(message: Message): HTMLElement;
  styled(tag: string, cls: string, text: string): HTMLElement;
  textBody(text: string): HTMLElement;
  anchorsIn(node: HTMLElement): { href: string; name: string }[];
  messageText(message: Message): string;
  messageLinks(message: Message): { href: string; name: string }[];
  renderBodyInto(parent: HTMLElement, messages: readonly Message[]): HTMLElement;
  restatedMarked(box: HTMLElement): boolean;
  headingPieces(heading: MessageHeading): HTMLElement;
  cardEllipsis(): HTMLElement;
  appendCardEllipsis(host: HTMLElement): void;
  prependCardEllipsis(host: HTMLElement): void;
  unlinkCardBody(row: HTMLElement): void;
}

/** The body builders over `deps`. */
export function createMessageBodies(deps: MessageBodyDependencies): MessageBodies {
  const wireMessageLinks = deps.wireMessageLinks;
  const textByMessage = new WeakMap<Message, string>();

function insertSanitizedHtml(parent: HTMLElement, contentHtml: string): void {
  const template = document.createElement("template");
  template.innerHTML = contentHtml;
  parent.append(...template.content.childNodes);
}

/**
 * Text as plain paragraphs: each run of lines that are not blank is a paragraph, and the lines in
 * it are separated by line breaks. Exactly the shape the server compares against before it decides
 * a body needs no HTML (`plain_html` in src/render.rs), so a message drawn this way looks as it
 * would have with its rendering. Every line through a text node.
 *
 * `#242 rich-headers`. With a `cut`, the first `cut` characters of the first line are the words a
 * heading restates, and are marked as the server marks them in a rendered body (`mark_restated` in
 * src/render.rs): in a `span.rh-src`, which also holds the line break after them when they are the
 * whole line, or as the paragraph's own class when that line is the whole paragraph. Marking adds
 * no character and moves none, so the body reads the same with the mark as without it.
 */

function plainParagraphs(parent: HTMLElement, text: string, cut = 0): void {
  let paragraph: HTMLElement | null = null;
  // The restated words, while they are the whole of the paragraph so far.
  let lead: HTMLElement | null = null;
  // The paragraph ends while it is only the restated line: the paragraph carries the mark itself.
  const close = () => {
    if (lead && paragraph) {
      paragraph.className = RESTATED_CLASS;
      paragraph.replaceChildren(...lead.childNodes);
    }
    lead = null;
    paragraph = null;
  };
  String(text).split("\n").forEach((line, index) => {
    if (/^[ \t]*$/.test(line)) {
      close();
      return;
    }
    if (paragraph) {
      (lead || paragraph).append(document.createElement("br"));
      lead = null;
    } else {
      paragraph = document.createElement("p");
      parent.append(paragraph);
    }
    if (index === 0 && cut > 0) {
      const words = document.createElement("span");
      words.className = RESTATED_CLASS;
      words.append(document.createTextNode(line.slice(0, cut)));
      paragraph.append(words);
      if (line.length > cut) paragraph.append(document.createTextNode(line.slice(cut)));
      else lead = words;
      return;
    }
    paragraph.append(document.createTextNode(line));
  });
  close();
}

/**
 * One message's body, as a `.md` box: the server's rendering when there is one, its text as plain
 * paragraphs otherwise. Every message on the page is drawn through here.
 */

function messageBody(message: Message): HTMLElement {
  const box = document.createElement("div");
  box.className = "md";
  const html = renderedHtmlOf(message);
  if (html !== "") {
    insertSanitizedHtml(box, html);
    // `#543 in-app-message-links`. A link to a message in a channel this app reads opens it here.
    wireMessageLinks(box);
  } else {
    const heading = headingOf(message);
    const cut = typeof (message as Message & { cardRestCut?: unknown }).cardRestCut === "number" ? ((message as Message & { cardRestCut?: unknown }).cardRestCut as number) : heading ? heading.cut : 0;
    plainParagraphs(box, rawTextOf(message), cut);
  }
  // `#528 thread-card-markdown-and-cap`. A split root's continuation (`continuationOf`): the same
  // body from the unit its card stopped before, formatted as the whole root is.
  if (typeof (message as Message & { cardRestFrom?: unknown }).cardRestFrom === "number") {
    const from = (message as Message & { cardRestFrom?: unknown }).cardRestFrom;
    keepCardUnits(box, (at: number) => at >= (from as number));
    prependCardEllipsis(box);
  }
  return box;
}

/** An element of `tag` holding `text`, assigned as text. */

function styled(tag: string, className: string, text: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  node.textContent = text;
  return node;
}

/** A body drawn from text alone: a draft being sent, which no server has rendered yet. */

function textBody(text: string): HTMLElement {
  const box = document.createElement("div");
  box.className = "md";
  plainParagraphs(box, text);
  return box;
}

/** Every link a drawn body holds, in order, as `{href, name}`: only the ones it really draws. */

function anchorsIn(node: HTMLElement): { href: string; name: string }[] {
  const found: { href: string; name: string }[] = [];
  const walk = (at: Node): void => {
    for (const kid of at.childNodes) {
      if (kid.nodeType !== 1) continue;
      const kidEl = kid as HTMLElement;
      const href = String(kidEl.tagName).toLowerCase() === "a" ? kidEl.getAttribute("href") : null;
      if (href && HTTP_ADDRESS.test(href)) {
        found.push({ href, name: renderedText(kidEl) || href });
      } else {
        walk(kid);
      }
    }
  };
  walk(node);
  return found;
}

/**
 * What one message says, as text, kept per message object: a thread's name asks on every draw of
 * the selector, and a row's message objects are replaced, never edited, when a read brings a newer
 * copy.
 */

function messageText(message: Message): string {
  if (!message || typeof message !== "object") return "";
  if (!textByMessage.has(message)) textByMessage.set(message, renderedText(messageBody(message)));
  return textByMessage.get(message) as string;
}

/**
 * The links one message draws. A rendered body's links are its anchors — what the reader can tap,
 * exactly — and a body drawn from text has none drawn, so its links are read from the text as the
 * voice transcript's are.
 */

function messageLinks(message: Message): { href: string; name: string }[] {
  return renderedHtmlOf(message) !== "" ? anchorsIn(messageBody(message)) : linksIn(rawTextOf(message));
}

/** Draw a row's messages into its body, one `.md` box each, in order. */

function renderBodyInto(parent: HTMLElement, messages: readonly Message[]): HTMLElement {
  for (const message of messages) parent.append(messageBody(message));
  return parent;
}

// --- rich headers ---------------------------------------------------------------------------------
//
// `#242 rich-headers`. The owner: "we have a lot of wasted space in the header of each message", and
// his agents open their messages with who they are — "[build-bot coordinator] Backup check, 10:52 PM
// ET. All 4 shards are synced." — because every agent posts through the one bot account. The server
// finds those pieces (src/heading.rs) and sends them as `heading`: a tag, a time and a title, and
// `cut`, how much of the message's text they restate. This page shows them in the row's header line
// and hides the words they came from.
//
//   * NOTHING IS LOST, EITHER WAY. The body is never cut. The restated words stay in it, marked
//     `rh-src` (by the server in a rendered body, by `plainParagraphs` in a plain one), and the
//     stylesheet hides them only while it shows the heading. With the setting off, the heading is
//     hidden and the words are drawn where they always were: the row is exactly what it was. The
//     switch is one attribute on the root, so flipping it redraws nothing and asks for nothing.
//   * THE TEXT IS UNTOUCHED. Copy, speech, search and a thread's name read the message as written,
//     or the drawn body as a whole, marked words included.
//   * WHERE IT GOES IS DECIDED BY THE STYLESHEET, from the list's width and a length class this page
//     sets from the strings alone — no layout is read (`#226`). A header that would not fit beside
//     the row's buttons is drawn as the first line of the message instead, in full: never cut short
//     with an ellipsis, which would drop words.
//   * ONLY THE ROW'S FIRST MESSAGE. A combined row (see `glom`) is headed by its first message; the
//     later ones keep their first lines as written.

/**
 * Show headings in the header, or draw every first line as written, and remember which. The root's
 * attribute is the whole of the switch: nothing else here asks whether it is on.
 */

function restatedMarked(box: HTMLElement): boolean {
  const first = box.childNodes[0] as HTMLElement | undefined;
  if (!first || first.nodeType !== 1 || String(first.tagName).toLowerCase() !== "p") return false;
  if (first.className === RESTATED_CLASS) return true;
  const lead = first.childNodes[0] as HTMLElement | undefined;
  return Boolean(lead && lead.nodeType === 1 && String(lead.tagName).toLowerCase() === "span" &&
    lead.className === RESTATED_CLASS);
}

/**
 * How long a heading reads, as the stylesheet's length class: its characters of tag, title and time,
 * rounded up to a multiple of eight from 16 to 96, or `long` past that. Counted from the strings,
 * never measured on the screen; web/voice.css says how wide a list each class needs. `extra` is the
 * characters the row's time says beyond the hour and minute the classes were measured with: its
 * weekday or date (`#729 vt-older-message-dates`), which take the same line.
 */

function headingPieces(heading: MessageHeading): HTMLElement {
  const head = document.createElement("span");
  head.className = "rh-head";
  if (heading.tag) head.append(styled("span", "rh-tag", heading.tag));
  if (heading.title) head.append(styled("span", "rh-title", heading.title));
  if (heading.time) head.append(styled("span", "rh-time", `@${heading.time}`));
  return head;
}

// --- who is speaking in the channel -------------------------------------------------------------
//
// Who is who — the reader's choices, everyone seen in each channel, the accounts this server posts
// as and the owner's own, and the bucket each author falls in — is web/src/core/channels's
// (`createIdentities`, `#624 voice-page-modularity`), with why there are four buckets and why they
// are keyed on the account. What is here is the page's: the redraw a newly learned account of ours
// needs, which channel a message is from, and Settings' list, which reads the owner.

/** Who is who. Nothing known until `load`, at start-up. */

const cardEllipsis = () => styled("span", "card-ellipsis", "…");

/** "…" at the end of `box`: on its last line, or on a line of its own after a block that has no line to end. */

function appendCardEllipsis(box: HTMLElement): void {
  const into = cardMarkTarget(box, true);
  if (into) {
    into.append(cardEllipsis());
    return;
  }
  const line = document.createElement("p");
  line.append(cardEllipsis());
  box.append(line);
}

/** "…" at the start of `box`, as `appendCardEllipsis` puts it at the end. */

function prependCardEllipsis(box: HTMLElement): void {
  const into = cardMarkTarget(box, false);
  if (into) {
    into.insertBefore(cardEllipsis(), into.firstChild);
    return;
  }
  const line = document.createElement("p");
  line.append(cardEllipsis());
  box.insertBefore(line, box.firstChild);
}

/** Every link in `node` drawn as a link's words, not a link: the card it is on is a button. */

function unlinkCardBody(node: Node): void {
  for (const kid of [...node.childNodes]) {
    if (kid.nodeType !== 1) continue;
    if (tagNameOf(kid) !== "a") {
      unlinkCardBody(kid);
      continue;
    }
    const words = document.createElement("span");
    words.className = "card-link";
    words.append(...kid.childNodes);
    kid.replaceWith(words);
  }
}

/**
 * Each root's cut as last worked out, the K it was worked out at, and the card's words for it: a card
 * asks several times a pass (its words, its mark, its block's signature and its block). A root read
 * again is a new object, and is worked out afresh.
 *
 * @type {WeakMap<object, {lines: number, cut: {head: number, whole: boolean} | null, text: string}>}
 */

  return {
    insertSanitizedHtml, plainParagraphs, messageBody, styled, textBody, anchorsIn,
    messageText, messageLinks, renderBodyInto, restatedMarked, headingPieces,
    cardEllipsis, appendCardEllipsis, prependCardEllipsis, unlinkCardBody,
  };
}

// The full-screen glossary (`#2233 vibe-talk-glossary-popup`): every known term, alphabetical, with a
// search field, opened by the Glossary button of the glossary popover — the same button whether the
// popover shows a WTF? answer or a tapped, already-defined term.
//
// THE CONTAINER. `#glossary-panel` stands beside `#discuss-panel` in `#screen-main` and is drawn by
// Discuss's own full-screen rules (22-discuss.css): the full sheet on a phone, the maximized panel on
// a desk, with Discuss's slim head. It stands above an open Discuss and never touches it.
//
// A MODAL. While it is up nothing under it takes a tap, a key or the focus: everything beside it, up to
// the body, is inert (`inertBeside`, the Recently-dismissed pop-out's rule, `#351 settings-polish`), the
// page's key dispatch stands down (`pageCovered` in web/src/app/keyboard.ts), and Tab goes round its own
// stops. Escape, its X and the phone's Back close it (`menu:glossary` in view/shell/navigation.ts),
// and the focus goes back to what opened the popover. Nothing under it moves: closing gives back
// exactly what was there, an open Discuss included, at the same scroll position.
//
// TEXT ONLY. Learned entries are model output: the term, the definition and the source label go in
// through `textContent`, never as markup, and a source becomes a link only when it is one of the
// server's in-app message addresses (`?…&m=<ref>`, routed through the page's jump) or a fetched
// document's key (opened in Documents). Anything else is shown as plain words.
//
// `#2234 vibe-talk-glossary-term-index` (not built here) adds label rows and a per-term mentions
// index on these same rows: each row is one `GlossaryTerm`, keyed by its normalized term
// (`data-term`).

import { deepLinkRefFrom } from "../../foundation/index.ts";
import type { MessageLinkRef } from "../../foundation/index.ts";
import { createListening } from "../dom/index.ts";
import type { ElementById } from "../dom/index.ts";

/** One known term as `GET /api/v1/glossary/terms` serves it. */
export interface GlossaryTerm {
  term: string;
  definition: string;
  source: { kind: string; href: string; label: string } | null;
  origin: string;
}

/** Where a glossary source may take the reader: an in-app message, a fetched document, or nowhere. */
export type GlossarySourceTarget =
  | { kind: "message"; href: string; ref: MessageLinkRef }
  | { kind: "document"; key: string }
  | null;

/** What the glossary popup asks of the page. */
export interface GlossaryPanelDependencies {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  /** The known terms, fresh from the server; rejects when they could not be read. */
  loadTerms(): Promise<GlossaryTerm[]>;
  /** Open a fetched document by key in the Documents view. */
  openDocument(key: string): void;
  /** Jump to a message through the page's link-jump path (Discuss's citations take the same one). */
  jumpToMessage(ref: MessageLinkRef): void;
  /** The back gesture's entries follow what is open. */
  scheduleBackSync?(): void;
}

/** The glossary popup's commands. */
export interface GlossaryPanel {
  /** Open it; scrolled to `term`'s entry and marked when given, and the focus back to `returnFocus` on close. */
  open(options?: { term?: string | null; returnFocus?: HTMLElement | null }): void;
  /** Close it, giving the focus back when `refocus` (the default); whether it was open. */
  close(refocus?: boolean): boolean;
  isOpen(): boolean;
  unmount(): void;
}

/** A term as the server compares terms: words joined by one space, lower case. */
export function normalizeGlossaryTerm(term: string): string {
  return String(term ?? "").split(/\s+/).filter(Boolean).join(" ").toLowerCase();
}

/** Alphabetical, as a reader expects: case and accents aside, digits by value; one entry per term. */
export function sortGlossaryTerms(terms: readonly GlossaryTerm[]): GlossaryTerm[] {
  const seen = new Set<string>();
  const unique: GlossaryTerm[] = [];
  for (const entry of terms) {
    if (!entry || typeof entry.term !== "string") continue;
    const norm = normalizeGlossaryTerm(entry.term);
    if (!norm || seen.has(norm)) continue;
    seen.add(norm);
    unique.push(entry);
  }
  return unique.sort((a, b) => {
    const order = a.term.localeCompare(b.term, undefined, { sensitivity: "base", numeric: true });
    return order !== 0 ? order : a.term < b.term ? -1 : a.term > b.term ? 1 : 0;
  });
}

/** The letter heading an entry stands under: its first letter, accents aside, or "#" for anything else. */
export function glossaryLetter(term: string): string {
  const first = String(term ?? "").trim().normalize("NFD").replace(/[\u0300-\u036f]/g, "").charAt(0).toUpperCase();
  return /^[A-Z]$/.test(first) ? first : "#";
}

/**
 * What the search keeps of sorted `terms`: those whose term holds `query`, then those only whose
 * definition does, each in alphabetical order. Case and runs of spaces aside; an empty query keeps all.
 */
export function filterGlossaryTerms(
  terms: readonly GlossaryTerm[],
  query: string,
): { termMatches: GlossaryTerm[]; definitionMatches: GlossaryTerm[] } {
  const wanted = normalizeGlossaryTerm(query);
  if (!wanted) return { termMatches: [...terms], definitionMatches: [] };
  const termMatches: GlossaryTerm[] = [];
  const definitionMatches: GlossaryTerm[] = [];
  for (const entry of terms) {
    if (normalizeGlossaryTerm(entry.term).includes(wanted)) termMatches.push(entry);
    else if (normalizeGlossaryTerm(entry.definition).includes(wanted)) definitionMatches.push(entry);
  }
  return { termMatches, definitionMatches };
}

/**
 * Where a source may take the reader, checked: a message source only when its address is the server's
 * in-app shape (`?…&m=<provider>:<channel>:<message>[:<thread>]`), a document source by its key, which
 * is opened in Documents and never put in an address. A `javascript:` address, an outside URL or anything
 * else as a message source is no target: its label stays words.
 */
export function glossarySourceTarget(source: GlossaryTerm["source"] | undefined): GlossarySourceTarget {
  if (!source || typeof source !== "object") return null;
  const href = typeof source.href === "string" ? source.href.trim() : "";
  if (!href) return null;
  if (source.kind === "document") {
    // A document's key (`github:<repo>@<ref>:<path>`, `paste:<id>`), never an address: it is only ever
    // asked of the documents route, never followed, so any key but one carrying control characters stands.
    if (/[\u0000-\u001f\u007f]/.test(href) || href.length > 2048) return null;
    return { kind: "document", key: href };
  }
  if (source.kind !== "message" || !href.startsWith("?")) return null;
  const params = new URLSearchParams(href.slice(1));
  const ref = deepLinkRefFrom(params.get("m") ?? "");
  if (ref === null) return null;
  return {
    kind: "message",
    href,
    ref: { service: ref.provider, channel: ref.channel, message: ref.message, thread: ref.thread },
  };
}

/** The words an entry's origin is shown as: reviewed by a person, or learned by a WTF? lookup. */
export function glossaryOriginLabel(origin: string): string {
  if (origin === "reviewed") return "Reviewed";
  if (origin === "dynamic") return "Learned";
  return "";
}

/** Whether `node` is `root` or inside it (the fixture DOM has no `contains`). */
function within(node: unknown, root: unknown): boolean {
  for (let at = node as { parentNode?: unknown } | null; at; at = (at.parentNode ?? null) as { parentNode?: unknown } | null) {
    if (at === root) return true;
  }
  return false;
}

/** Whether `node` or anything around it up to `root` is hidden. */
function hiddenWithin(node: HTMLElement, root: HTMLElement): boolean {
  for (let at: HTMLElement | null = node; at && at !== root; at = at.parentNode as HTMLElement | null) {
    if (at.hidden) return true;
  }
  return false;
}

/**
 * Make everything on the page but `node` inert: each element beside it, and beside every element around
 * it, up to the body; what it made inert is returned for the close to give back, and an element already
 * inert is left to whatever made it so. What stands in the flow outside `#screen-main` (the top bar,
 * the dock) is dimmed too (`data-under-dim`, 02-body.css), so no control a tap cannot reach looks as if
 * it could be pressed; what floats is covered by the popup or left as it is. The Recently-dismissed
 * pop-out's rule (`inertAround`, view/settings/dismissed.ts), kept here so each modal owns its own.
 */
function inertBeside(node: Element, screen: Element | null): Element[] {
  const made: Element[] = [];
  let beyond = false;
  for (let at = node; at !== document.body && at.parentNode; at = at.parentNode as Element) {
    if (at === screen) beyond = true;
    const parent = at.parentNode as Element;
    for (const beside of Array.from(parent.children ?? [])) {
      if (beside === at || beside.hasAttribute("inert")) continue;
      beside.setAttribute("inert", "");
      if (beyond && !floats(beside)) beside.setAttribute("data-under-dim", "");
      made.push(beside);
    }
  }
  return made;
}

/** Whether `node` is taken out of the flow (it floats over the screen rather than standing beside it). */
function floats(node: Element): boolean {
  if (typeof window.getComputedStyle !== "function") return false;
  const position = window.getComputedStyle(node).position;
  return position === "absolute" || position === "fixed";
}

/** Mount the full-screen glossary over `#glossary-panel`. */
export function createGlossaryPanel(deps: GlossaryPanelDependencies): GlossaryPanel {
  const { listen, unmount } = createListening();
  const panel = () => deps.byId("glossary-panel");
  const search = () => deps.byId("glossary-panel-search");
  const list = () => deps.byId("glossary-panel-list");

  let terms: GlossaryTerm[] = [];
  let state: "loading" | "ready" | "failed" = "loading";
  let current: string | null = null;
  let returnFocus: HTMLElement | null = null;
  let madeInert: Element[] = [];
  let generation = 0;

  function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
    const made = document.createElement(tag);
    made.className = className;
    if (text !== undefined) made.textContent = text;
    return made;
  }

  function sourceLine(source: GlossaryTerm["source"]): HTMLElement | null {
    if (!source || typeof source !== "object") return null;
    const label = String(source.label ?? "").trim() || "source";
    const line = element("p", "glossary-entry-source");
    line.append(document.createTextNode("Source: "));
    const target = glossarySourceTarget(source);
    if (target && target.kind === "message") {
      const link = element("a", "glossary-entry-link", label);
      link.setAttribute("href", target.href);
      link.setAttribute("data-glossary-source", "message");
      line.append(link);
    } else if (target && target.kind === "document") {
      const button = element("button", "glossary-entry-link", label);
      button.setAttribute("type", "button");
      button.setAttribute("data-glossary-source", "document");
      button.setAttribute("data-glossary-doc", target.key);
      line.append(button);
    } else {
      line.append(element("span", "glossary-entry-source-text", label));
    }
    return line;
  }

  function entryRow(entry: GlossaryTerm): HTMLElement {
    const norm = normalizeGlossaryTerm(entry.term);
    const row = element("li", norm === current ? "glossary-entry glossary-entry-current" : "glossary-entry");
    row.setAttribute("data-term", norm);
    if (norm === current) row.setAttribute("aria-current", "true");
    const head = element("p", "glossary-entry-head");
    head.append(element("span", "glossary-entry-term", entry.term));
    const origin = glossaryOriginLabel(entry.origin);
    if (origin) {
      const badge = element("span", "glossary-entry-badge", origin);
      badge.setAttribute("data-origin", entry.origin);
      head.append(document.createTextNode(" "), badge);
    }
    row.append(head);
    row.append(element("p", "glossary-entry-definition", String(entry.definition ?? "")));
    const line = sourceLine(entry.source);
    if (line) row.append(line);
    return row;
  }

  function group(heading: string, entries: readonly GlossaryTerm[]): HTMLElement[] {
    const title = element("h3", "glossary-letter", heading);
    const rows = element("ul", "glossary-entries");
    for (const entry of entries) rows.append(entryRow(entry));
    return [title, rows];
  }

  function say(words: string, retry = false): void {
    const line = deps.byId("glossary-panel-state");
    line.textContent = words;
    line.hidden = words === "";
    deps.byId("glossary-panel-retry").hidden = !retry;
  }

  function render(): void {
    const box = list();
    const count = deps.byId("glossary-panel-count");
    if (state === "loading") {
      box.replaceChildren();
      count.textContent = "";
      say("Loading the glossary…");
      return;
    }
    if (state === "failed") {
      box.replaceChildren();
      count.textContent = "";
      say("Couldn't load the glossary. Check the connection and try again.", true);
      return;
    }
    const total = terms.length;
    if (total === 0) {
      box.replaceChildren();
      count.textContent = "No terms";
      say("No glossary entries yet. Select words in a message and press WTF? to learn one.");
      return;
    }
    const query = String(search().value ?? "");
    const { termMatches, definitionMatches } = filterGlossaryTerms(terms, query);
    const shown = termMatches.length + definitionMatches.length;
    const noun = total === 1 ? "term" : "terms";
    count.textContent = normalizeGlossaryTerm(query) ? `${shown} of ${total} ${noun}` : `${total} ${noun}`;
    if (shown === 0) {
      box.replaceChildren();
      say(`No terms match “${query.trim()}”.`);
      return;
    }
    say("");
    const parts: HTMLElement[] = [];
    if (normalizeGlossaryTerm(query)) {
      if (termMatches.length) {
        const rows = element("ul", "glossary-entries");
        for (const entry of termMatches) rows.append(entryRow(entry));
        parts.push(rows);
      }
      if (definitionMatches.length) parts.push(...group("In definitions", definitionMatches));
    } else {
      let letter = "";
      let bucket: GlossaryTerm[] = [];
      for (const entry of termMatches) {
        const at = glossaryLetter(entry.term);
        if (at !== letter && bucket.length) {
          parts.push(...group(letter, bucket));
          bucket = [];
        }
        letter = at;
        bucket.push(entry);
      }
      if (bucket.length) parts.push(...group(letter, bucket));
    }
    box.replaceChildren(...parts);
  }

  /** Bring the marked entry to the popup's middle, moving only the popup's own scroll. */
  function showCurrent(): void {
    if (!current) return;
    const row = Array.from(list().querySelectorAll("li")).find((one) => one.getAttribute("data-term") === current);
    if (!row) return;
    const box = panel();
    const rowRect = row.getBoundingClientRect();
    const boxRect = box.getBoundingClientRect();
    const delta = rowRect.top - boxRect.top - Math.max(0, (boxRect.height - rowRect.height) / 2);
    if (Number.isFinite(delta) && delta !== 0) box.scrollTop = Math.max(0, box.scrollTop + delta);
  }

  async function load(): Promise<void> {
    const mine = ++generation;
    state = "loading";
    render();
    try {
      const fresh = await deps.loadTerms();
      if (mine !== generation) return;
      terms = sortGlossaryTerms(Array.isArray(fresh) ? fresh : []);
      state = "ready";
    } catch {
      if (mine !== generation) return;
      state = "failed";
    }
    render();
    showCurrent();
  }

  function deskLayout(): boolean {
    const width = document.documentElement?.clientWidth ?? 0;
    return width === 0 || width >= 900;
  }

  function isOpen(): boolean {
    return !panel().hidden;
  }

  function open(options: { term?: string | null; returnFocus?: HTMLElement | null } = {}): void {
    const wasOpen = isOpen();
    current = options.term ? normalizeGlossaryTerm(options.term) : null;
    if (!wasOpen) returnFocus = options.returnFocus ?? null;
    search().value = "";
    const box = panel();
    box.scrollTop = 0;
    box.hidden = false;
    if (!wasOpen) {
      const screen = document.getElementById("screen-main");
      madeInert = inertBeside(box, screen);
    }
    // A desk types at once; a phone's keyboard would cover half the list, so there the focus goes to
    // the popup itself and the search field is one tap away.
    const focusTarget = deskLayout() ? search() : box;
    if (typeof focusTarget.focus === "function") focusTarget.focus({ preventScroll: true });
    deps.scheduleBackSync?.();
    void load();
  }

  function close(refocus = true): boolean {
    if (!isOpen()) return false;
    generation += 1;
    panel().hidden = true;
    // The page behind it back first: an inert element cannot take the focus.
    for (const node of madeInert) {
      node.removeAttribute("inert");
      node.removeAttribute("data-under-dim");
    }
    madeInert = [];
    const back = returnFocus;
    returnFocus = null;
    current = null;
    if (refocus && back && !back.hidden && typeof back.focus === "function") back.focus({ preventScroll: true });
    deps.scheduleBackSync?.();
    return true;
  }

  /** The popup's Tab stops in order: what can take the focus and is shown. */
  function stops(): HTMLElement[] {
    const root = panel();
    const found: HTMLElement[] = [];
    const walk = (node: HTMLElement): void => {
      for (const kid of Array.from(node.children ?? []) as HTMLElement[]) {
        if (kid.hidden) continue;
        const tag = String(kid.tagName || "").toUpperCase();
        if ((tag === "BUTTON" || tag === "INPUT" || (tag === "A" && kid.getAttribute("href"))) && !hiddenWithin(kid, root)) {
          found.push(kid);
        }
        walk(kid);
      }
    };
    walk(root);
    return found;
  }

  listen<MouseEvent>(deps.byId("glossary-panel-close"), "click", () => close());
  listen<MouseEvent>(deps.byId("glossary-panel-retry"), "click", () => void load());
  listen<Event>(search(), "input", () => {
    current = null;
    render();
  });
  listen<MouseEvent>(list(), "click", (event) => {
    for (let at = (event?.target ?? null) as HTMLElement | null; at && at !== (list() as unknown); at = at.parentNode as HTMLElement | null) {
      const kind = typeof at.getAttribute === "function" ? at.getAttribute("data-glossary-source") : null;
      if (kind === "document") {
        event?.preventDefault?.();
        const key = at.getAttribute("data-glossary-doc") || "";
        close(false);
        if (key) deps.openDocument(key);
        return;
      }
      if (kind === "message") {
        event?.preventDefault?.();
        const target = glossarySourceTarget({ kind: "message", href: at.getAttribute("href") || "", label: "" });
        close(false);
        if (target && target.kind === "message") deps.jumpToMessage(target.ref);
        return;
      }
    }
  });
  listen<KeyboardEvent>(document, "keydown", (event) => {
    if (!isOpen()) return;
    if (event.key === "Escape") {
      // Taken here, so the page's own Escape (which stands down while the popup covers it) never
      // goes on to close what is under it.
      if (typeof event.preventDefault === "function") event.preventDefault();
      if (typeof event.stopPropagation === "function") event.stopPropagation();
      close();
      return;
    }
    if (event.key !== "Tab") return;
    const all = stops();
    if (!all.length) return;
    const focused = document.activeElement;
    const at = all.indexOf(focused as HTMLElement);
    const inside = within(focused, panel());
    let next: HTMLElement | null = null;
    if (!inside || at < 0) next = event.shiftKey ? all[all.length - 1]! : all[0]!;
    else if (event.shiftKey && at === 0) next = all[all.length - 1]!;
    else if (!event.shiftKey && at === all.length - 1) next = all[0]!;
    if (next) {
      if (typeof event.preventDefault === "function") event.preventDefault();
      next.focus({ preventScroll: true });
    }
  });

  return { open, close, isOpen, unmount };
}

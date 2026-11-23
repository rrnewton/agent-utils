// WTF glossary lookup (`#2156 vibe-talk-wtf-glossary-lookup`): the WTF? button left of the
// Quoted reply chip, the popover it opens, and the glossary links known terms render as.
// The button shows exactly while the selection chip does (it reads the chip's own place each
// frame the selection moves), so the two can never disagree about whether a selection stands;
// the dismiss X stays on the chip's right, untouched. Learned entries are popover-only: they
// are stored by the server, shown here and linked below, and never fed to the Discuss warm-up.

import { createListening } from "../dom/index.ts";
import { setHiddenIfChanged } from "../dom/index.ts";
import type { ElementById } from "../dom/index.ts";

/** One known term as `GET /api/v1/glossary/terms` serves it. */
export interface GlossaryTerm {
  term: string;
  definition: string;
  source: { kind: string; href: string; label: string } | null;
  origin: string;
}

/** What the glossary view asks of the page. */
export interface GlossaryDependencies {
  byId<K extends keyof ElementById>(id: K): ElementById[K];
  api(path: string, options?: { method?: string; body?: unknown }): Promise<unknown>;
  selectedChannelId(): string;
  glossaryAvailable(): boolean;
  /** The ids of a row's message or messages, or none (`idsOf`). */
  idsOf(row: Element | null | undefined): string[];
  /** Open a fetched document by key in the Documents view. */
  openDocument(key: string): void;
}

/** The glossary view's commands. */
export interface GlossaryView {
  /** Fetch the known terms and link them in the lists. */
  refreshTerms(): Promise<void>;
  /** Take down the listeners this mount added. */
  unmount(): void;
}

// Placement edge in CSS pixels (kept literal: the page constants census
// in tests/js/page covers named tunables, and this is not one).

function normalize(term: string): string {
  return term.split(/\s+/).filter(Boolean).join(" ").toLowerCase();
}

/** Mount the WTF? button, popover and glossary linking. */
export function createGlossaryView(deps: GlossaryDependencies): GlossaryView {
  const { listen, unmount } = createListening();
  let terms: GlossaryTerm[] = [];
  let byNorm = new Map<string, GlossaryTerm>();
  let anchor: { top: number; left: number; bottom: number } | null = null;

  const wtf = () => deps.byId("selection-chip-wtf");
  const popover = () => deps.byId("glossary-popover");

  function placeWtf(): void {
    const chip = deps.byId("selection-chip");
    const button = wtf();
    if (!deps.glossaryAvailable() || chip.hidden) {
      setHiddenIfChanged(button, true);
      return;
    }
    setHiddenIfChanged(button, false);
    const rect = chip.getBoundingClientRect();
    const size = button.getBoundingClientRect();
    const width = Number(size.width) || 64;
    const viewWidth = (document.documentElement && document.documentElement.clientWidth) || 0;
    let left = rect.left - 8 - width;
    if (left < 8) left = Math.min(rect.right + 8, viewWidth - 8 - width);
    button.style.left = `${Math.round(Math.max(8, left))}px`;
    button.style.top = `${Math.round(rect.top)}px`;
  }

  function placePopover(): void {
    const box = popover();
    if (!anchor) return;
    const size = box.getBoundingClientRect();
    const width = Number(size.width) || 300;
    const height = Number(size.height) || 120;
    const viewWidth = (document.documentElement && document.documentElement.clientWidth) || 0;
    const viewHeight = window.innerHeight || 0;
    const left = Math.max(8, Math.min(anchor.left, viewWidth - 8 - width));
    let top = anchor.bottom + 8;
    if (top + height > viewHeight - 8) top = Math.max(8, anchor.top - height - 8);
    box.style.left = `${Math.round(left)}px`;
    box.style.top = `${Math.round(top)}px`;
  }

  function showPopover(
    term: string,
    body: string,
    source: GlossaryTerm["source"],
    savedNote: boolean,
  ): void {
    deps.byId("glossary-popover-term").textContent = term;
    deps.byId("glossary-popover-body").textContent = savedNote
      ? `${body} Saved to the glossary.`
      : body;
    const sourceLine = deps.byId("glossary-popover-source");
    const link = deps.byId("glossary-popover-link");
    if (source) {
      link.textContent = source.label;
      if (source.kind === "document") {
        link.setAttribute("href", "#");
        link.dataset.glossaryDoc = source.href;
        link.removeAttribute("data-msg");
      } else {
        link.setAttribute("href", source.href);
        link.removeAttribute("data-glossary-doc");
      }
      setHiddenIfChanged(sourceLine, false);
    } else {
      setHiddenIfChanged(sourceLine, true);
    }
    setHiddenIfChanged(popover(), false);
    placePopover();
  }

  function closePopover(): void {
    setHiddenIfChanged(popover(), true);
  }

  function selectionInfo(): { text: string; messageId: string; context: string } | null {
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
    const text = sel.toString().split(/\s+/).filter(Boolean).join(" ").trim();
    if (!text) return null;
    const range = sel.getRangeAt(0);
    let node: Node | null = range.startContainer;
    let row: Element | null = null;
    while (node) {
      if (node.nodeType === 1) {
        const element = node as Element;
        if (element.tagName === "LI" || element.getAttribute("role") === "row") {
          row = element;
          break;
        }
      }
      node = node.parentNode;
    }
    const ids = row ? deps.idsOf(row) : [];
    const rect = range.getBoundingClientRect();
    anchor = { top: rect.top, left: rect.left, bottom: rect.bottom };
    return {
      text,
      messageId: ids.length ? String(ids[0]) : "",
      context: row ? (row.textContent || "").split(/\s+/).join(" ").slice(0, 500) : text,
    };
  }

  async function pressWtf(): Promise<void> {
    const info = selectionInfo();
    const channel = deps.selectedChannelId();
    if (!info || !channel) return;
    anchor = anchor ?? { top: 80, left: 16, bottom: 120 };
    showPopover(info.text, "Looking this up…", null, false);
    try {
      const answer = (await deps.api(
        `/api/v1/channels/${encodeURIComponent(channel)}/glossary/lookup`,
        {
          method: "POST",
          body: { selected: info.text, message_id: info.messageId || null, context: info.context },
        },
      )) as {
        state?: string;
        term?: string;
        definition?: string | null;
        source?: GlossaryTerm["source"];
        reason?: string | null;
        saved?: boolean;
      };
      if (answer && answer.state === "found" && answer.definition) {
        showPopover(
          String(answer.term || info.text),
          String(answer.definition),
          answer.source ?? null,
          answer.saved === true,
        );
        void refreshTerms();
      } else {
        showPopover(
          info.text,
          `Couldn't find what “${info.text}” means in recent messages or fetched documents.`,
          null,
          false,
        );
      }
    } catch {
      showPopover(info.text, "The lookup failed — check the connection and try again.", null, false);
    }
  }

  function linkTermsIn(root: ParentNode): void {
    if (!terms.length) return;
    const walker = document.createTreeWalker(root as Node, NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        if (!node.textContent || !node.textContent.trim()) return NodeFilter.FILTER_REJECT;
        let at: Node | null = node.parentNode;
        while (at && at !== root) {
          if (at.nodeType === 1) {
            const element = at as Element;
            const tag = element.tagName;
            if (tag === "A" || tag === "CODE" || tag === "PRE" || tag === "BUTTON") {
              return NodeFilter.FILTER_REJECT;
            }
            if (element.classList.contains("glossary-term")) return NodeFilter.FILTER_REJECT;
          }
          at = at.parentNode;
        }
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    const nodes: Text[] = [];
    for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node as Text);
    for (const node of nodes) {
      const text = node.textContent || "";
      const lower = text.toLowerCase();
      let best: { at: number; entry: GlossaryTerm } | null = null;
      for (const entry of terms) {
        const at = lower.indexOf(normalize(entry.term));
        if (at >= 0 && (!best || at < best.at || (at === best.at && entry.term.length > best.entry.term.length))) {
          best = { at, entry };
        }
      }
      if (!best) continue;
      const { at, entry } = best;
      const before = text.slice(0, at);
      const matched = text.slice(at, at + entry.term.length);
      const after = text.slice(at + entry.term.length);
      const button = document.createElement("button");
      button.type = "button";
      button.className = "glossary-term";
      button.textContent = matched;
      button.dataset.glossaryTerm = entry.term;
      const parent = node.parentNode;
      if (!parent) continue;
      if (after) parent.insertBefore(document.createTextNode(after), node.nextSibling);
      parent.insertBefore(button, node.nextSibling);
      node.textContent = before;
      // One link per text node per pass; later passes catch the rest.
      void linkTermsIn(parent as unknown as ParentNode);
    }
  }

  async function refreshTerms(): Promise<void> {
    try {
      const answer = (await deps.api("/api/v1/glossary/terms", { method: "GET" })) as {
        terms?: GlossaryTerm[];
      };
      const list = answer && Array.isArray(answer.terms) ? answer.terms : [];
      terms = [...list].sort((a, b) => b.term.length - a.term.length);
      byNorm = new Map(terms.map((entry) => [normalize(entry.term), entry]));
      const log = document.getElementById("discord-log");
      if (log) linkTermsIn(log);
      const pinned = document.getElementById("pinned-log");
      if (pinned) linkTermsIn(pinned);
    } catch {
      // No term list: nothing links, the WTF? press still answers for itself.
    }
  }

  // Terms load lazily on first selection, never at page load: a page that
  // only reads must not spend a glossary read (or any read with its token).
  let termsAsked = false;
  function ensureTerms(): void {
    if (termsAsked) return;
    termsAsked = true;
    void refreshTerms();
  }

  listen<Event>(document, "selectionchange", () => {
    ensureTerms();
    if (typeof requestAnimationFrame === "function") {
      // Twice: the Quote chip places itself on the first frame after a
      // selection, so the WTF? button settles against its final place.
      requestAnimationFrame(() => requestAnimationFrame(placeWtf));
    } else placeWtf();
  });
  listen<MouseEvent>(wtf(), "click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    void pressWtf();
  });
  listen<MouseEvent>(deps.byId("glossary-popover-close"), "click", () => closePopover());
  listen<MouseEvent>(deps.byId("glossary-popover-link"), "click", (event) => {
    const key = (event.currentTarget as HTMLElement).dataset.glossaryDoc;
    if (key) {
      event.preventDefault();
      closePopover();
      deps.openDocument(key);
    }
  });
  listen<KeyboardEvent>(document, "keydown", (event) => {
    if (event.key === "Escape") closePopover();
  });
  listen<MouseEvent>(document, "click", (event) => {
    const target = event.target as Element | null;
    const termButton =
      target && typeof target.closest === "function" ? target.closest(".glossary-term") : null;
    if (termButton) {
      const held = termButton as HTMLElement;
      const entry = byNorm.get(normalize(held.dataset.glossaryTerm || held.textContent || ""));
      if (entry) {
        const rect = termButton.getBoundingClientRect();
        anchor = { top: rect.top, left: rect.left, bottom: rect.bottom };
        showPopover(entry.term, entry.definition, entry.source, false);
      }
      return;
    }
    if (!popover().hidden && target && !popover().contains(target) && target !== wtf()) {
      closePopover();
    }
  });

  const log = document.getElementById("discord-log");
  let scheduled = false;
  const observer =
    typeof MutationObserver === "function" && log
      ? new MutationObserver(() => {
          if (scheduled) return;
          scheduled = true;
          if (typeof requestAnimationFrame === "function") {
            requestAnimationFrame(() => {
              scheduled = false;
              if (log) linkTermsIn(log);
            });
          } else {
            scheduled = false;
            linkTermsIn(log);
          }
        })
      : null;
  if (observer && log) observer.observe(log, { childList: true, subtree: true });

  return {
    refreshTerms,
    unmount() {
      observer?.disconnect();
      unmount();
    },
  };
}

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
  /** Whether the page's token scope is known (a refused token reads nothing). */
  tokenKnown(): boolean;
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
    // `#2195 vibe-talk-wtf-chip-orphan-lifecycle`. A collapsed selection takes WTF? down at once:
    // never re-show it against a chip the quoting view is about to hide.
    const selNow = typeof getSelection === "function" ? getSelection() : null;
    if (!deps.glossaryAvailable() || chip.hidden || (selNow && selNow.isCollapsed)) {
      setHiddenIfChanged(button, true);
      return;
    }
    // `#2220 vibe-talk-selection-chip-rigid-row`. WTF? is never positioned here: the quoting view
    // positions the one container, and this view only toggles WTF?'s visibility inside its reserved
    // slot. Revealing it cannot move the container or its siblings.
    setHiddenIfChanged(button, false);
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
    // `#2188 vibe-talk-wtf-popover-below-chip`. Below the chip it spawned from, never over it: when the
    // WTF? chip stands, the popover starts under the chip (and under the selection), whichever is lower.
    const chipButton = wtf();
    const chipRect = chipButton && !chipButton.hidden ? chipButton.getBoundingClientRect() : null;
    let top = anchor.bottom + 8;
    if (chipRect) top = Math.max(top, chipRect.bottom + 8);
    if (top + height > viewHeight - 8) {
      // No room below: above the selection, and still never over the chip.
      const aboveFrom = chipRect ? Math.min(anchor.top, chipRect.top) : anchor.top;
      top = Math.max(8, aboveFrom - height - 8);
    }
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
    // `#2195 vibe-talk-wtf-chip-orphan-lifecycle`. Closing the popover takes the WTF? chip down
    // with it; the quoting view's one show/hide path owns the rest of the group.
    setHiddenIfChanged(wtf(), true);
  }

  function selectionInfo(
    given?: Range,
  ): { text: string; messageId: string; context: string } | null {
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (!given && (!sel || sel.isCollapsed || !sel.rangeCount)) return null;
    const range = given ?? (sel as Selection).getRangeAt(0);
    const text = range.toString().split(/\s+/).filter(Boolean).join(" ").trim();
    if (!text) return null;
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

  // The last live selection's range, kept (unread) because a phone tap on
  // the button can clear the browser selection before the press reads it
  // (the same hold Quote reply keeps). Its words are read once, at press.
  let lastRange: Range | null = null;

  async function pressWtf(): Promise<void> {
    const info = selectionInfo() ?? (lastRange ? selectionInfo(lastRange) : null);
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
        await refreshTerms();
        const log = document.getElementById("discord-log");
        if (log) linkTermsIn(log, true);
      } else {
        const searched = answer && typeof answer.reason === "string" ? answer.reason : "";
        showPopover(
          info.text,
          searched
            ? `Couldn't find what “${info.text}” means. ${searched}`
            : `Couldn't find what “${info.text}” means in recent messages or fetched documents.`,
          null,
          false,
        );
      }
    } catch {
      showPopover(info.text, "The lookup failed — check the connection and try again.", null, false);
    }
  }

  function linkTermsIn(root: ParentNode, force = false): void {
    if (!terms.length) return;
    // While any selection stands, linking anywhere risks the list rebuild
    // it triggers collapsing that selection; the press re-links on landing.
    if (!force) {
      const liveSel = typeof getSelection === "function" ? getSelection() : null;
      if (liveSel && !liveSel.isCollapsed) return;
    }
    // Never rewrite the row the reader is selecting in: replacing its text
    // nodes would collapse the selection the WTF? press stands on. `force`
    // is the press having landed: the popover stands on its own now.
    let selectedRow: Element | null = null;
    const live = typeof getSelection === "function" ? getSelection() : null;
    if (!force && live && !live.isCollapsed && live.rangeCount) {
      let at: Node | null = live.getRangeAt(0).startContainer;
      while (at) {
        if (at.nodeType === 1 && (at as Element).tagName === "LI") {
          selectedRow = at as Element;
          break;
        }
        at = at.parentNode;
      }
    }
    const walker = document.createTreeWalker(root as Node, NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        if (!node.textContent || !node.textContent.trim()) return NodeFilter.FILTER_REJECT;
        if (selectedRow && selectedRow.contains(node)) return NodeFilter.FILTER_REJECT;
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
    if (termsAsked || !deps.tokenKnown() || !deps.glossaryAvailable()) return;
    termsAsked = true;
    void refreshTerms();
  }

  listen<Event>(document, "selectionchange", () => {
    ensureTerms();
    // Keep the range unread: a drag must not read the words at every move
    // (Quote reply reads them once, at press; so does this).
    const sel = typeof getSelection === "function" ? getSelection() : null;
    if (sel && !sel.isCollapsed && sel.rangeCount) {
      try {
        lastRange = sel.getRangeAt(0).cloneRange();
      } catch {
        lastRange = null;
      }
    }
    // A collapsed selection frees the row it stood in for linking.
    if (sel && sel.isCollapsed && terms.length) {
      const log = document.getElementById("discord-log");
      if (log) linkTermsIn(log);
    }
    // `#2220`. No re-alignment pass: visibility only, in the same frame as the selection change.
    placeWtf();
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

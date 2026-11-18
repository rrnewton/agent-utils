// The search bar's Labels drop-down (`#1853 vibe-talk-labels-dropdown-pinned-builtin`):
// the pinned-threads button grown into a label picker. Pinned is the built-in
// first label (the channel's pin set, gold as ever); hashtag labels and label
// groups follow most-used first from the same `GET /api/v1/labels` read
// Settings > Labels browses, so the two surfaces can never disagree. Single-
// select: picking a row filters to it, picking it again (or All messages)
// clears; AND stays a typed `label:a label:b`. The filter in force is also
// derivable from the page (`pinnedOnly`, else a lone `label:<slug>` query),
// which is what the closed button names.

import { childByClass, createListening } from "../dom/index.ts";

/** One drop-down row as Settings' labels browser serves it (its own
 * `LabelDropdownEntry` in view/settings, kept structurally: view/channel
 * may not import view/settings, and the page's glue joins the two). */
export interface LabelDropdownEntry {
  readonly name: string;
  readonly count: number;
  readonly grouped: boolean;
  readonly spellings: readonly string[];
}

/** What the drop-down asks of the page. */
export interface LabelsFilterDependencies {
  byId(id: string): HTMLElement;
  /** The drop-down rows, most used first (Settings' collapsed entries). */
  entries(): LabelDropdownEntry[];
  /** Re-read the labels from the server (Settings' read), then redrawn. */
  refreshLabels(): Promise<void>;
  /** This channel's pins: how many, and whether the list has been read.
   * Reading it is the Pinned selection's own act (`setPinnedOnly`), never
   * the panel's opening, so opening costs no read. */
  pinCount(): number;
  pinLoaded(): boolean;
  /** Whether the Pinned list stands in place of the timeline. */
  pinnedOnly(): boolean;
  setPinnedOnly(on: boolean): void;
  /** The search field's words, for naming and clearing a label filter. */
  query(): string;
  /** Open the search filtered to `label`, the card chips' own act. */
  filterToLabel(label: string): void;
  /** Take a lone `label:<slug>` query away, the bar staying open. */
  clearLabelQuery(): void;
  /** Run an async read with its failures caught quietly. */
  guard(run: () => Promise<void>): void;
}

/** The drop-down's commands. */
export interface LabelsFilter {
  /** Whether the panel stands open. */
  open(): boolean;
  /** Open or close the panel; closing returns focus to the button. */
  setOpen(open: boolean): void;
  /** Restate the closed button (its word and accessible name) from the page. */
  syncButton(): void;
  /** Take every listener down again. */
  unmount(): void;
}

/** The label a query of exactly `label:<slug>` names, else null. */
export function loneLabelOf(query: string): string | null {
  const match = /^label:([a-z0-9_-]+)$/i.exec(query.trim());
  return match ? match[1]!.toLowerCase() : null;
}

/** Closed, nothing selected but what the page already filters to. */
export function createLabelsFilter(deps: LabelsFilterDependencies): LabelsFilter {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  let isOpen = false;
  /** The row the keyboard stands on, by option value; "" for none yet. */
  let activeValue = "";
  /** Option values in drawn order, All first, for the keyboard. */
  let drawnValues: string[] = [];

  /** The filter in force: "pinned", a hashtag slug, or null. */
  function activeLabel(): string | null {
    if (deps.pinnedOnly()) return "pinned";
    return loneLabelOf(deps.query());
  }

  function syncButton(): void {
    const button = el("labels-button");
    const active = activeLabel();
    const word = childByClass(button, "labels-filter-word");
    if (word) {
      word.textContent = active === "pinned" ? "Pinned" : active ? `#${active}` : "Labels";
    }
    button.setAttribute(
      "aria-label",
      active
        ? `Labels: ${active === "pinned" ? "Pinned" : `#${active}`} selected. Filter by label`
        : "Labels. Filter by label",
    );
    if (active) button.setAttribute("data-active-label", active);
    else button.removeAttribute("data-active-label");
  }

  /** Every row the panel can show for `filter`, All and Pinned unfilterable. */
  function rows(filter: string): Array<{ value: string; name: string; count: string; pinned: boolean; grouped: boolean }> {
    const needle = filter.trim().toLowerCase();
    const out: Array<{ value: string; name: string; count: string; pinned: boolean; grouped: boolean }> = [
      { value: "all", name: "All messages", count: "", pinned: false, grouped: false },
      {
        value: "pinned",
        name: "Pinned",
        count: deps.pinLoaded() ? `${deps.pinCount()} in this channel` : "in this channel",
        pinned: true,
        grouped: false,
      },
    ];
    for (const entry of deps.entries()) {
      if (needle && !entry.name.includes(needle) && !entry.spellings.some((s) => s.includes(needle))) continue;
      out.push({
        value: entry.name,
        name: `#${entry.name}${entry.grouped ? " (G)" : ""}`,
        count: `${entry.count} in loaded history`,
        pinned: false,
        grouped: entry.grouped,
      });
    }
    return out;
  }

  function draw(): void {
    const input = el("labels-filter-input") as HTMLInputElement;
    const shown = rows(input.value);
    drawnValues = shown.map((row) => row.value);
    if (!drawnValues.includes(activeValue)) activeValue = activeLabel() ?? "all";
    const active = activeLabel();
    const list = el("labels-listbox");
    list.replaceChildren(
      ...shown.map((row) => {
        const item = document.createElement("li");
        item.className = row.pinned ? "labels-option labels-option-pinned" : "labels-option";
        item.id = `labels-option-${row.value}`;
        item.setAttribute("role", "option");
        item.setAttribute("aria-selected", String(active === row.value || (row.value === "all" && active === null)));
        item.setAttribute(
          "aria-label",
          row.value === "all" ? "All messages" : `${row.name}, ${row.count}`,
        );
        item.tabIndex = row.value === activeValue ? 0 : -1;
        if (row.value === activeValue) item.setAttribute("data-active", "");
        const name = document.createElement("span");
        name.className = "labels-option-name";
        name.textContent = row.name;
        item.append(name);
        if (row.count) {
          const count = document.createElement("span");
          count.className = "labels-option-count";
          count.textContent = row.count;
          item.append(count);
        }
        // Direct, not delegated: the page tests' DOM does not bubble, and a
        // delegated handler would be no handler there.
        listen(item, "click", () => select(row.value));
        return item;
      }),
    );
    const onlyBuiltIns = shown.length <= 2;
    el("labels-panel-state").textContent =
      deps.entries().length === 0
        ? "No hashtag labels in the loaded history yet. #pinned is reserved for Pinned."
        : onlyBuiltIns && input.value.trim() !== ""
          ? "No labels match that filter."
          : "";
    syncButton();
  }

  function focusValue(value: string): void {
    activeValue = value;
    draw();
    el(`labels-option-${value}`)?.focus();
  }

  function select(value: string): void {
    const active = activeLabel();
    setOpen(false);
    if (value === "all" || value === active) {
      if (deps.pinnedOnly()) deps.setPinnedOnly(false);
      if (loneLabelOf(deps.query()) !== null) deps.clearLabelQuery();
    } else if (value === "pinned") {
      if (loneLabelOf(deps.query()) !== null) deps.clearLabelQuery();
      deps.setPinnedOnly(true);
    } else {
      if (deps.pinnedOnly()) deps.setPinnedOnly(false);
      deps.filterToLabel(value);
    }
    syncButton();
    el("labels-button").focus();
  }

  function setOpen(open: boolean): void {
    isOpen = open;
    el("labels-panel").hidden = !open;
    el("labels-button").setAttribute("aria-expanded", String(open));
    if (open) {
      draw();
      deps.guard(async () => {
        await deps.refreshLabels();
        if (isOpen) draw();
      });
      (el("labels-filter-input") as HTMLInputElement).focus();
    }
  }

  // The panel's own visibility is the truth the click toggles: the control
  // bar hides the panel with its button (view change), which must not leave
  // the next click closing a panel that is already gone.
  listen(el("labels-button"), "click", () => setOpen(el("labels-panel").hidden));
  listen(el("labels-filter-input"), "input", () => {
    activeValue = "";
    draw();
  });
  listen(el("labels-filter-input"), "keydown", (event: KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (drawnValues.length > 0) focusValue(drawnValues[0]!);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const first = drawnValues.find((value) => value !== "all") ?? drawnValues[0];
      if (first) select(first);
    } else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      el("labels-button").focus();
    }
  });
  listen(el("labels-listbox"), "keydown", (event: KeyboardEvent) => {
    const index = drawnValues.indexOf(activeValue);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = event.key === "ArrowDown"
        ? Math.min(index + 1, drawnValues.length - 1)
        : Math.max(index - 1, 0);
      if (drawnValues[next]) focusValue(drawnValues[next]);
    } else if (event.key === "Home") {
      event.preventDefault();
      if (drawnValues[0]) focusValue(drawnValues[0]);
    } else if (event.key === "End") {
      event.preventDefault();
      const last = drawnValues[drawnValues.length - 1];
      if (last) focusValue(last);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (activeValue) select(activeValue);
    } else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      el("labels-button").focus();
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  });
  return {
    open: () => isOpen,
    setOpen,
    syncButton,
    unmount,
  };
}

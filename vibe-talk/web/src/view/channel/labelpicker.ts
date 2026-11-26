// The label picker (`#2279 vibe-talk-post-facto-labels`, owner requests 380 and 382): label a
// message after the fact, from its ⋯ menu ("Add label…") or the phone's long-press sheet
// ("Label…"). ONE label set: a name beginning with `#` is the hashtag label a typed `#kvm` makes
// (same chip, drop-down row, `label:` search and Settings row); any other name is a plain label.
//
// Pinned is the built-in first row, toggling the message's pin (`#1853`): pin and labels in one
// place, as the Labels drop-down has them. Then the labels already known, most used first and
// filtered as you type (the same `GET /api/v1/labels` read the drop-down and Settings make), and
// "Add new label …" when the typed name is none of them. The picker stays open after an add, so
// several labels go on in a row. A label the menu added comes off again (× or a tap on its ✓); a
// label the message TEXT carries stays while the text has it, and the picker says so.
//
// It stands in the top layer (`popover`), fixed, so opening it — and the phone keyboard its field
// opens — never reflows or scrolls the list under it (owner rule 370): under the row's ⋯ on a
// desktop, full width at the top of the visible viewport on a phone, its own list scrolling
// within what the keyboard leaves.

import type { Message } from "../../contract/index.ts";
import { checkLabelName, labelNamesOf } from "../../core/channels/index.ts";
import type { LabelState } from "../../core/channels/index.ts";
import { createListening } from "../dom/index.ts";

/** One known label, as the drop-down lists it (kept structurally, as `LabelDropdownEntry` is). */
export interface LabelPickerEntry {
  /** A hashtag label's slug or a group's display (no `#`), or a plain label's name. */
  readonly name: string;
  readonly count: number;
  /** A plain label (no `#`). Absent or false for a hashtag label or group. */
  readonly plain?: boolean;
}

/** What a label write answers (`MessageLabelChangeResponse`), as far as the picker reads it. */
export interface LabelWriteAnswer {
  readonly event: LabelState & { readonly channel_id: string; readonly message_id: string; readonly revision: number };
  readonly name: string;
  readonly changed: boolean;
  readonly text_derived?: boolean;
}

/** What the picker asks of the page. */
export interface LabelPickerDependencies {
  byId(id: string): HTMLElement;
  /** The known labels, most used first. */
  entries(): LabelPickerEntry[];
  /** Re-read the known labels from the server (the drop-down's read). */
  refreshLabels(): Promise<void>;
  /** The channel the row's messages are in. */
  channel(): string;
  /** The page's current copy of a message (`fallback` when it holds none), the labels this
   * page last applied to it folded in: the picker re-reads after each write. */
  held(id: string, fallback: Message): Message;
  /** Whether any of the messages is pinned, and toggling it (`#206`). */
  isPinned(messages: Message[]): boolean;
  togglePin(messages: Message[]): Promise<unknown>;
  /** `PUT` (`present`) or `DELETE` one label of one message, the page's exact request. */
  write(channel: string, messageId: string, name: string, present: boolean): Promise<LabelWriteAnswer>;
  /** Fold a write's answer into every held copy and redraw its row, keeping the place. */
  apply(answer: LabelWriteAnswer): void;
  /** Whether the browser says it is online: offline, nothing is written and the picker says so,
   * as Pin does (`navigator.onLine !== false`). */
  online(): boolean;
  /** Whether the page is laid out for a phone (one column, touch): the picker hangs full width. */
  narrow(): boolean;
  /** The visible viewport (what the keyboard leaves), for placing on a phone. */
  visibleViewport(): { top: number; height: number; width: number };
  /** Run an async act with its failures caught, saying them on the status line. */
  guard(run: () => Promise<void>): void;
}

/** The picker's commands. */
export interface LabelPicker {
  /** Open for the row's `messages`, placed from `anchor`, returning focus there on close. */
  open(messages: Message[], row: HTMLElement, anchor: HTMLElement): void;
  close(): void;
  isOpen(): boolean;
  /** Draw again if open: a live event or a re-read changed what the messages carry. */
  refresh(): void;
  unmount(): void;
}

/** One list row: the built-in, a known label, or a new name to add. */
interface PickerRow {
  readonly value: string;
  readonly name: string;
  readonly kind: "pinned" | "label" | "new";
  readonly on: boolean;
  readonly count?: number;
}

type TopLayer = HTMLElement & { showPopover?: () => void; hidePopover?: () => void; matches?: (selector: string) => boolean };

/** Closed. */
export function createLabelPicker(deps: LabelPickerDependencies): LabelPicker {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  let ids: string[] = [];
  let originals: Message[] = [];
  let anchorEl: HTMLElement | null = null;
  let opened = false;
  let activeValue = "";
  let drawnValues: string[] = [];
  let note = "";
  let busy = false;

  /** The row's messages as the store holds them now. */
  function messages(): Message[] {
    return ids.map((id, at) => deps.held(id, originals[at] as Message));
  }

  /** Every label name on the row, and which of them the menu added (removable). */
  function onRow(): { names: string[]; manual: Set<string> } {
    const names: string[] = [];
    const manual = new Set<string>();
    for (const message of messages()) {
      for (const name of labelNamesOf(message as never)) if (!names.includes(name)) names.push(name);
      for (const name of ((message as { manual_labels?: string[] }).manual_labels || [])) manual.add(name);
    }
    return { names, manual };
  }

  function rows(): PickerRow[] {
    const input = el("label-picker-input") as HTMLInputElement;
    const typed = input.value;
    const needle = typed.trim().toLowerCase().replace(/^#/, "");
    const { names } = onRow();
    const out: PickerRow[] = [];
    if (!needle || "pinned".includes(needle)) {
      out.push({ value: "pinned", name: "Pinned", kind: "pinned", on: deps.isPinned(messages()) });
    }
    const known = new Set<string>();
    for (const entry of deps.entries()) {
      const name = entry.plain ? entry.name : `#${entry.name}`;
      known.add(name);
      if (needle && !entry.name.includes(needle)) continue;
      out.push({ value: name, name, kind: "label", on: names.includes(name), count: entry.count });
    }
    // The row's own labels the browse does not list (outside the loaded history's counts).
    for (const name of names) {
      if (known.has(name) || (needle && !name.toLowerCase().includes(needle))) continue;
      out.push({ value: name, name, kind: "label", on: true });
    }
    const checked = typed.trim() ? checkLabelName(typed) : null;
    if (checked && checked.name !== undefined && !out.some((row) => row.value === checked.name)) {
      out.splice(out[0]?.kind === "pinned" ? 1 : 0, 0, { value: `new:${checked.name}`, name: checked.name, kind: "new", on: false });
    }
    return out;
  }

  function draw(): void {
    if (!opened) return;
    const input = el("label-picker-input") as HTMLInputElement;
    // Offline nothing can be written: the field looks inactive so it does not invite typing,
    // and says so to assistive technology (`#2276` review of the offline picker).
    input.setAttribute("aria-disabled", deps.online() ? "false" : "true");
    const typed = input.value.trim();
    const checked = typed ? checkLabelName(typed) : null;
    el("label-picker-error").textContent = checked && checked.error !== undefined ? checked.error : "";
    // The labels on the message, as removable chips (the menu's) or plain ones (the text's).
    const { names, manual } = onRow();
    const current = el("label-picker-current");
    const textCarried: string[] = [];
    current.replaceChildren(...names.map((name) => {
      const chip = document.createElement("span");
      chip.className = name.startsWith("#") ? "label-picker-chip" : "label-picker-chip label-picker-chip-plain";
      const word = document.createElement("span");
      word.textContent = name;
      chip.append(word);
      if (manual.has(name)) {
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "label-picker-remove";
        remove.textContent = "×";
        remove.setAttribute("aria-label", `Remove ${name}`);
        remove.title = `Remove ${name} from this message`;
        listen(remove, "click", () => toggle(name, false));
        chip.append(remove);
      } else {
        textCarried.push(name);
      }
      return chip;
    }));
    current.hidden = names.length === 0;
    el("label-picker-note").textContent = note || (textCarried.length
      ? `${textCarried.join(", ")} ${textCarried.length === 1 ? "is" : "are"} in the message text, so ${textCarried.length === 1 ? "it stays" : "they stay"} while the text has ${textCarried.length === 1 ? "it" : "them"}.`
      : "");
    const shown = rows();
    drawnValues = shown.map((row) => row.value);
    if (!drawnValues.includes(activeValue)) activeValue = "";
    const list = el("label-picker-list");
    list.replaceChildren(...shown.map((row, at) => {
      const item = document.createElement("li");
      item.className = `label-picker-option label-picker-${row.kind}`;
      item.id = `label-picker-option-${at}`;
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(row.on));
      item.setAttribute("data-value", row.value);
      item.tabIndex = row.value === activeValue ? 0 : -1;
      if (row.value === activeValue) item.setAttribute("data-active", "");
      const mark = document.createElement("span");
      mark.className = "label-picker-check";
      mark.setAttribute("aria-hidden", "true");
      mark.textContent = row.on ? "✓" : "";
      const name = document.createElement("span");
      name.className = "label-picker-name";
      name.textContent = row.kind === "new" ? `Add new label ${row.name}` : row.name;
      item.append(mark, name);
      if (row.count !== undefined) {
        const count = document.createElement("span");
        count.className = "label-picker-count";
        count.textContent = String(row.count);
        count.title = `${row.count} in loaded history`;
        item.append(count);
      }
      item.setAttribute("aria-label", row.kind === "new" ? `Add new label ${row.name}` :
        row.kind === "pinned" ? (row.on ? "Pinned, on: unpin" : "Pinned: pin this message") :
        `${row.name}${row.on ? ", on this message" : ""}${row.count !== undefined ? `, ${row.count} in loaded history` : ""}`);
      // Direct, not delegated: the page tests' DOM does not bubble.
      listen(item, "click", () => pick(row));
      return item;
    }));
    place();
  }

  function pick(row: PickerRow): void {
    if (busy) return;
    if (row.kind === "pinned") {
      deps.guard(async () => {
        busy = true;
        try {
          await deps.togglePin(messages());
        } finally {
          busy = false;
          draw();
        }
      });
      return;
    }
    const { manual } = onRow();
    if (row.on) {
      if (manual.has(row.value)) toggle(row.value, false);
      else {
        note = `${row.value} is in the message text, so it stays while the text has it.`;
        draw();
      }
      return;
    }
    toggle(row.kind === "new" ? row.name : row.value, true);
  }

  /** Add `name` to the row's first message, or remove it from every message the menu added it to. */
  function toggle(name: string, present: boolean): void {
    if (busy) return;
    note = "";
    // Offline: nothing is sent and nothing is queued, and the reader is told — Pin's way.
    if (!deps.online()) {
      note = `Offline — nothing was ${present ? "added" : "removed"}. Try again once the connection is back.`;
      draw();
      return;
    }
    const held = messages();
    // The message's own channel: ids are per channel.
    const channel = String((held[0] as { channel_id?: unknown } | undefined)?.channel_id || deps.channel());
    const targets = present
      ? [held[0]].filter(Boolean) as Message[]
      : held.filter((message) => ((message as { manual_labels?: string[] }).manual_labels || []).includes(name));
    if (targets.length === 0) return;
    busy = true;
    deps.guard(async () => {
      try {
        for (const message of targets) {
          const answer = await deps.write(channel, String(message.id), name, present);
          deps.apply(answer);
          if (!present && answer.text_derived) note = `${answer.name} is in the message text, so it stays while the text has it.`;
        }
        if (present) (el("label-picker-input") as HTMLInputElement).value = "";
      } catch (error) {
        // The server's refusal (a name it will not take, a bound), said where the owner looks.
        const refusal = error as { detail?: string; message?: string };
        note = refusal.detail || refusal.message || "The server did not answer.";
      } finally {
        busy = false;
        draw();
        deps.guard(async () => {
          await deps.refreshLabels();
          draw();
        });
      }
    });
  }

  /** Place the picker: full width at the top of what the keyboard leaves on a phone, under its anchor on a desktop. */
  function place(): void {
    const picker = el("label-picker");
    if (typeof picker.getBoundingClientRect !== "function" || typeof window === "undefined") return;
    const edge = 8;
    const view = deps.visibleViewport();
    if (deps.narrow() || !anchorEl || typeof anchorEl.getBoundingClientRect !== "function") {
      picker.style.left = `${edge}px`;
      picker.style.right = `${edge}px`;
      picker.style.width = "auto";
      picker.style.top = `${view.top + edge}px`;
      picker.style.maxHeight = `${Math.max(120, view.height - edge * 2)}px`;
      return;
    }
    const box = anchorEl.getBoundingClientRect();
    const width = Math.min(352, view.width - edge * 2);
    const left = Math.min(Math.max(box.right - width, edge), view.width - width - edge);
    picker.style.width = `${width}px`;
    picker.style.right = "auto";
    picker.style.left = `${left}px`;
    const below = box.bottom + 4;
    const room = view.top + view.height - below - edge;
    const height = picker.getBoundingClientRect().height;
    const top = room >= Math.min(height, 240) ? below : Math.max(view.top + edge, box.top - 4 - height);
    picker.style.top = `${top}px`;
    picker.style.maxHeight = `${Math.max(120, view.top + view.height - top - edge)}px`;
  }

  function open(rowMessages: Message[], _row: HTMLElement, anchor: HTMLElement): void {
    ids = rowMessages.map((message) => String(message.id));
    originals = [...rowMessages];
    anchorEl = anchor;
    note = deps.online() ? "" : "Offline — labels can be added or removed once the connection is back.";
    activeValue = "";
    opened = true;
    const picker = el("label-picker") as TopLayer;
    (el("label-picker-input") as HTMLInputElement).value = "";
    picker.hidden = false;
    if (typeof picker.showPopover === "function") {
      try {
        if (!(typeof picker.matches === "function" && picker.matches(":popover-open"))) picker.showPopover();
      } catch {
        // Shown by `hidden` alone where the top layer is not available.
      }
    }
    draw();
    // A phone opens its keyboard only when the field is tapped: picking a known label needs none.
    if (!deps.narrow()) (el("label-picker-input") as HTMLInputElement).focus({ preventScroll: true });
    else el("label-picker-close").focus({ preventScroll: true });
    deps.guard(async () => {
      await deps.refreshLabels();
      draw();
    });
  }

  function close(): void {
    if (!opened) return;
    opened = false;
    const picker = el("label-picker") as TopLayer;
    if (typeof picker.hidePopover === "function") {
      try {
        if (typeof picker.matches === "function" && picker.matches(":popover-open")) picker.hidePopover();
      } catch {
        // Already down.
      }
    }
    picker.hidden = true;
    const back = anchorEl;
    anchorEl = null;
    if (back && typeof back.focus === "function" && back.isConnected !== false) back.focus({ preventScroll: true });
  }

  function focusValue(value: string): void {
    activeValue = value;
    draw();
    const at = drawnValues.indexOf(value);
    if (at >= 0) el(`label-picker-option-${at}`)?.focus({ preventScroll: true });
  }

  listen(el("label-picker-close"), "click", () => close());
  listen(el("label-picker-input"), "input", () => {
    activeValue = "";
    note = "";
    draw();
  });
  listen(el("label-picker-input"), "keydown", (event: KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (drawnValues.length > 0) focusValue(drawnValues[0]!);
    } else if (event.key === "Enter") {
      event.preventDefault();
      // The new name if one is offered, else the first label (never the built-in by Enter).
      const shown = rows();
      const first = shown.find((row) => row.kind === "new") || shown.find((row) => row.kind === "label" && !row.on);
      if (first) pick(first);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  });
  listen(el("label-picker-list"), "keydown", (event: KeyboardEvent) => {
    const index = drawnValues.indexOf(activeValue);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = event.key === "ArrowDown" ? Math.min(index + 1, drawnValues.length - 1) : Math.max(index - 1, 0);
      if (drawnValues[next] !== undefined) focusValue(drawnValues[next]!);
    } else if (event.key === "Home") {
      event.preventDefault();
      if (drawnValues[0] !== undefined) focusValue(drawnValues[0]!);
    } else if (event.key === "End") {
      event.preventDefault();
      const last = drawnValues[drawnValues.length - 1];
      if (last !== undefined) focusValue(last);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const row = rows().find((one) => one.value === activeValue);
      if (row) pick(row);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  });

  return {
    open,
    close,
    isOpen: () => opened,
    refresh: () => draw(),
    unmount,
  };
}

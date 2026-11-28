// The search bar's dates (`#2425 vibe-talk-search-date-filter`, owner 2026-11-27: "The search
// interface should support "after:" like gmail and remind me the syntax. But also there should be a
// calendar icon in the search bar and clicking it should provide an option for last K days or since
// DATE (e.g. date range where you don't have to fill in both)").
//
// The field's text is the one source of truth: the calendar never holds a date of its own. Its panel
// READS the after: / before: tokens standing in the text (to pre-fill and to name the button), and
// WRITES its choice back as tokens — an absolute day, computed when written, so a recalled search
// means the days it meant when it ran (the liaison's settled Q2) — replacing only the plain,
// top-level date tokens it owns and keeping every other word as typed. The text then runs through the
// one filter change typing makes (`applyText`), so the server, the loaded rows, the count and the
// bucket chips all see the date exactly as if typed. The hint under the bar reminds the syntax with
// a real example, or names the date standing.

import { createListening } from "../dom/index.ts";

/** A calendar day, as the search language names one. */
export interface DatesDay {
  readonly year: number;
  readonly month: number;
  readonly day: number;
}

/** What the dates control asks of the page. */
export interface SearchDatesDependencies {
  byId(id: string): HTMLElement;
  /** The reader's calendar day, in the zone the page searches in. */
  today(): DatesDay;
  /**
   * The day a `before:` / `after:` value names, by the shared search language
   * (`searchDateValue`): a calendar date, or with `today` a day counted back (`7d`); else null.
   */
  dateValue(value: string, today: DatesDay | null): DatesDay | null;
  /** The field's text, as it stands. */
  query(): string;
  /** Put `text` in the field and run it as typed: the one filter change typing makes. */
  applyText(text: string): void;
  /** Close the Labels panel (the two hang in one place; one stands at a time). */
  closeLabels(): void;
}

/** The dates control's commands. */
export interface SearchDates {
  /** Whether the panel stands open. */
  open(): boolean;
  /** Open or close the panel; closing returns focus to the field. */
  setOpen(open: boolean): void;
  /** Restate the button's state and the hint from the field's text. */
  sync(): void;
  /** Take every listener down again. */
  unmount(): void;
}

/** One `after:` / `before:` token standing in a query's text. */
export interface DateToken {
  /** Where it starts and ends in the text (end exclusive). */
  readonly start: number;
  readonly end: number;
  readonly field: "after" | "before";
  /** Its value as typed (a quoted value without its quotes). */
  readonly value: string;
  /** Whether it is the calendar's to replace: plain (no `-`), at the top level (not in a group nor beside OR). */
  readonly owned: boolean;
}

/**
 * Every `after:` / `before:` token of `text`, outside quoted phrases, with whether the calendar owns
 * it. Validity of the value is the caller's question (the language's `dateValue`).
 */
export function dateTokens(text: string): DateToken[] {
  type Raw = { start: number; end: number; word: string; value?: string };
  const words: Raw[] = [];
  let at = 0;
  while (at < text.length) {
    const character = text[at]!;
    if (/\s/.test(character)) {
      at += 1;
      continue;
    }
    if (character === "\"") {
      const close = text.indexOf("\"", at + 1);
      const end = close < 0 ? text.length : close + 1;
      words.push({ start: at, end, word: text.slice(at, end) });
      at = end;
      continue;
    }
    const start = at;
    while (at < text.length && !/\s/.test(text[at]!) && text[at] !== "\"") at += 1;
    const word = text.slice(start, at);
    // A field with nothing after its colon takes the phrase that follows (`after:"2026-11-20"`).
    if (/^-?(after|before):$/i.test(word) && text[at] === "\"") {
      const close = text.indexOf("\"", at + 1);
      const end = close < 0 ? text.length : close + 1;
      words.push({ start, end, word, value: text.slice(at + 1, close < 0 ? text.length : close) });
      at = end;
      continue;
    }
    words.push({ start, end: at, word });
  }
  const out: DateToken[] = [];
  words.forEach((raw, index) => {
    const match = /^(-*)(after|before):(.*)$/i.exec(raw.word);
    if (!match) return;
    const value = raw.value ?? match[3]!;
    if (!value) return;
    const beside = (word: Raw | undefined): boolean => word !== undefined && word.word === "OR";
    const owned = match[1] === "" && !beside(words[index - 1]) && !beside(words[index + 1]);
    out.push({ start: raw.start, end: raw.end, field: match[2]!.toLowerCase() as "after" | "before", value, owned });
  });
  return out;
}

/** `text` with the tokens at `drop` taken out and the spaces they leave closed up. */
function without(text: string, drop: readonly DateToken[]): string {
  let out = text;
  for (const token of [...drop].sort((a, b) => b.start - a.start)) {
    out = `${out.slice(0, token.start)} ${out.slice(token.end)}`;
  }
  return out.replace(/\s+/g, " ").trim();
}

const order = (day: DatesDay): number => day.year * 10_000 + day.month * 100 + day.day;

/** The day `count` days before `day` (`count` may be negative: after). */
export function daysBefore(day: DatesDay, count: number): DatesDay {
  const at = new Date(0);
  at.setUTCFullYear(day.year, day.month - 1, day.day - count);
  return { year: at.getUTCFullYear(), month: at.getUTCMonth() + 1, day: at.getUTCDate() };
}

/** Whole days from `from` to `to`. */
function daysBetween(from: DatesDay, to: DatesDay): number {
  return Math.round((Date.UTC(to.year, to.month - 1, to.day) - Date.UTC(from.year, from.month - 1, from.day)) / 86_400_000);
}

/** `YYYY-MM-DD`, as the field and a date box write a day. */
export function dayText(day: DatesDay): string {
  return `${String(day.year).padStart(4, "0")}-${String(day.month).padStart(2, "0")}-${String(day.day).padStart(2, "0")}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "Nov 20", or "Nov 20, 2025" outside `today`'s year; `withYear` always names the year. */
function dayWords(day: DatesDay, today: DatesDay, withYear = false): string {
  const short = `${MONTHS[day.month - 1]} ${day.day}`;
  return withYear || day.year !== today.year ? `${short}, ${day.year}` : short;
}

/** The dates a query's own text stands on, as the calendar owns them. */
export interface StandingDates {
  /** The latest owned `after:` day, and the value it was typed as. */
  readonly after: DatesDay | null;
  readonly afterValue: string;
  /** The earliest owned `before:` day (exclusive). */
  readonly before: DatesDay | null;
  /** The owned tokens holding a valid date: what the calendar replaces. */
  readonly tokens: readonly DateToken[];
}

/** Read the owned date tokens of `text` (`dateValue` judges each value, `today` reads `7d`). */
export function standingDates(
  text: string,
  today: DatesDay,
  dateValue: (value: string, today: DatesDay | null) => DatesDay | null,
): StandingDates {
  let after: DatesDay | null = null;
  let afterValue = "";
  let before: DatesDay | null = null;
  const tokens: DateToken[] = [];
  for (const token of dateTokens(text)) {
    if (!token.owned) continue;
    const day = dateValue(token.value, today);
    if (!day) continue;
    tokens.push(token);
    if (token.field === "after") {
      if (!after || order(day) >= order(after)) {
        after = day;
        afterValue = token.value;
      }
    } else if (!before || order(day) < order(before)) {
      before = day;
    }
  }
  return { after, afterValue, before, tokens };
}

/** Whether a typed value is a relative day (`7d`, `today`, `yesterday`): it moves with today. */
const relative = (value: string): boolean => /^([0-9]{1,5}d|today|yesterday)$/i.test(value);

/** What the dates standing say, short: "since Nov 20", "the last 7 days", "Nov 10 – Nov 19", or null. */
export function datesWords(standing: StandingDates, today: DatesDay, withYear = false): string | null {
  const { after, before } = standing;
  if (after && before) {
    // The last day named is the one before before:, Between's inclusive end; one year named once.
    const last = daysBefore(before, 1);
    const first = after.year === last.year ? dayWords(after, { ...today, year: after.year }) : dayWords(after, today, withYear);
    return `${first} – ${dayWords(last, today, withYear)}`;
  }
  if (after) {
    if (relative(standing.afterValue)) {
      const back = daysBetween(after, today);
      return back === 0 ? "today" : back === 1 ? "since yesterday" : `last ${back} days`;
    }
    return `since ${dayWords(after, today, withYear)}`;
  }
  if (before) return `before ${dayWords(before, today, withYear)}`;
  return null;
}

/** The panel's choice, read from its controls. */
type Mode = "any" | "last" | "since" | "before" | "between";

/** Mount the calendar button, its panel and the hint. */
export function createSearchDates(deps: SearchDatesDependencies): SearchDates {
  const { listen, unmount } = createListening();
  const el = (id: string): HTMLElement => deps.byId(id);
  const input = (id: string): HTMLInputElement => el(id) as HTMLInputElement;
  const MODE_IDS: Record<Mode, string> = {
    any: "search-dates-any",
    last: "search-dates-last",
    since: "search-dates-since-mode",
    before: "search-dates-before-mode",
    between: "search-dates-between-mode",
  };

  const standingNow = (): StandingDates => standingDates(deps.query(), deps.today(), deps.dateValue);

  function sync(): void {
    const today = deps.today();
    const standing = standingNow();
    const words = datesWords(standing, today);
    const full = datesWords(standing, today, true);
    const button = el("search-dates");
    const name = `Dates: ${full ?? "any time"}`;
    if (button.getAttribute("aria-label") !== name) {
      button.setAttribute("aria-label", name);
      button.setAttribute("title", name);
    }
    if (button.getAttribute("aria-pressed") !== String(words !== null)) button.setAttribute("aria-pressed", String(words !== null));
    // The hint: the syntax with a real example, or the date standing (Q4: it stays while the bar is
    // open). A typed relative day says that it moves with today.
    // A typed relative day moves with today; the short standing says it only in its title (R5).
    const hint = el("search-date-hint");
    const text = words === null
      ? `after:${dayText(daysBefore(today, 7))} · after:7d · before:${dayText(today)}`
      : words.charAt(0).toUpperCase() + words.slice(1);
    const moving = standing.after !== null && !standing.before && relative(standing.afterValue);
    if (hint.textContent !== text) hint.textContent = text;
    const title = words === null ? `Dates: ${text}` : `Dates: ${full}${moving ? ", moving with today" : ""}`;
    if (hint.getAttribute("title") !== title) hint.setAttribute("title", title);
  }

  function mode(): Mode {
    for (const [value, id] of Object.entries(MODE_IDS) as Array<[Mode, string]>) {
      if (input(id).checked) return value;
    }
    return "any";
  }

  function setMode(value: Mode): void {
    for (const [each, id] of Object.entries(MODE_IDS) as Array<[Mode, string]>) input(id).checked = each === value;
  }

  /** A date box's day: "" is none; a value that is no real date is refused with its words. */
  function boxDay(id: string): { day: DatesDay | null; error: string | null } {
    const raw = input(id).value.trim();
    if (raw === "") return { day: null, error: null };
    const day = deps.dateValue(raw, null);
    return day ? { day, error: null } : { day: null, error: `${raw} isn't a date` };
  }

  /** The tokens the panel's choice writes, or the reason it cannot. */
  function choice(): { tokens: string[]; error: string | null } {
    const today = deps.today();
    switch (mode()) {
      case "any":
        return { tokens: [], error: null };
      case "last": {
        const raw = input("search-dates-days").value.trim();
        if (!/^[0-9]{1,5}$/.test(raw)) return { tokens: [], error: "Days must be a whole number" };
        return { tokens: [`after:${dayText(daysBefore(today, Number(raw)))}`], error: null };
      }
      case "since": {
        const { day, error } = boxDay("search-dates-since");
        if (error) return { tokens: [], error };
        return day ? { tokens: [`after:${dayText(day)}`], error: null } : { tokens: [], error: "Choose a date" };
      }
      case "before": {
        const { day, error } = boxDay("search-dates-before");
        if (error) return { tokens: [], error };
        return day ? { tokens: [`before:${dayText(day)}`], error: null } : { tokens: [], error: "Choose a date" };
      }
      case "between": {
        const from = boxDay("search-dates-from");
        const to = boxDay("search-dates-to");
        const error = from.error ?? to.error;
        if (error) return { tokens: [], error };
        if (!from.day && !to.day) return { tokens: [], error: "Choose a first or last day" };
        if (from.day && to.day && order(from.day) > order(to.day)) return { tokens: [], error: "The first day is after the last" };
        // Between names its last day inclusively; before: names the first day left out.
        const tokens: string[] = [];
        if (from.day) tokens.push(`after:${dayText(from.day)}`);
        if (to.day) tokens.push(`before:${dayText(daysBefore(to.day, -1))}`);
        return { tokens, error: null };
      }
    }
  }

  /** Say what Use dates will write, or why it cannot; mark the K chip that matches. */
  function preview(): void {
    const { tokens, error } = choice();
    const errorLine = el("search-dates-error");
    errorLine.hidden = error === null;
    errorLine.textContent = error ?? "";
    el("search-dates-preview").textContent = error !== null
      ? ""
      : tokens.length > 0 ? `Searches ${tokens.join(" ")}` : "Searches any time";
    const days = input("search-dates-days").value.trim();
    for (const k of ["7", "30", "90"]) {
      el(`search-dates-k${k}`).setAttribute("aria-pressed", String(mode() === "last" && days === k));
    }
  }

  /** Fill the panel from the dates standing in the text, so reopening never blanks it. */
  function prefill(): void {
    const today = deps.today();
    const standing = standingNow();
    const { after, before } = standing;
    for (const id of ["search-dates-since", "search-dates-before", "search-dates-from", "search-dates-to"]) input(id).value = "";
    if (after && before) {
      setMode("between");
      input("search-dates-from").value = dayText(after);
      input("search-dates-to").value = dayText(daysBefore(before, 1));
    } else if (after && relative(standing.afterValue)) {
      setMode("last");
      input("search-dates-days").value = String(daysBetween(after, today));
    } else if (after) {
      setMode("since");
      input("search-dates-since").value = dayText(after);
    } else if (before) {
      setMode("before");
      input("search-dates-before").value = dayText(before);
    } else {
      setMode("any");
    }
    preview();
  }

  function focusField(): void {
    const field = input("search-field");
    field.focus();
    try {
      field.setSelectionRange(field.value.length, field.value.length);
    } catch {
      // A field type without a caret keeps its own.
    }
  }

  function setOpen(open: boolean): void {
    const panel = el("search-dates-panel");
    panel.hidden = !open;
    el("search-dates").setAttribute("aria-expanded", String(open));
    if (open) {
      deps.closeLabels();
      prefill();
      input(MODE_IDS[mode()]).focus();
    }
  }

  /** Write the panel's choice into the text (or take the dates out), run it, and close. */
  function apply(tokens: readonly string[]): void {
    const standing = standingNow();
    const rest = without(deps.query(), standing.tokens);
    const text = [rest, ...tokens].filter((part) => part !== "").join(" ");
    setOpen(false);
    if (text !== deps.query()) deps.applyText(text);
    sync();
    focusField();
  }

  // The panel's own visibility is the truth a click toggles, as the Labels panel's is: the control
  // bar hides the panel with its button, which must not leave the next click closing nothing.
  listen(el("search-dates"), "click", () => setOpen(el("search-dates-panel").hidden));
  // The hint is the detail one tap away (R5): a tap on it opens the panel, as the calendar does.
  listen(el("search-date-hint"), "click", () => setOpen(true));
  // The Labels panel hangs in the same place: opening it puts this one away.
  listen(el("labels-button"), "click", () => {
    if (!el("search-dates-panel").hidden) setOpen(false);
  });
  for (const [value, id] of Object.entries(MODE_IDS) as Array<[Mode, string]>) {
    listen(el(id), "change", () => {
      setMode(value);
      preview();
    });
  }
  // A date box or the number of days chooses its own row as it is used.
  const ROW_OF: Array<[string, Mode]> = [
    ["search-dates-days", "last"],
    ["search-dates-since", "since"],
    ["search-dates-before", "before"],
    ["search-dates-from", "between"],
    ["search-dates-to", "between"],
  ];
  for (const [id, value] of ROW_OF) {
    const choose = (): void => {
      setMode(value);
      preview();
    };
    listen(el(id), "input", choose);
    listen(el(id), "change", choose);
    listen<KeyboardEvent>(el(id), "keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        const { tokens, error } = choice();
        if (error === null) apply(tokens);
        else preview();
      }
    });
  }
  // Last 7, 30 or 90: the number of days, chosen; Use dates writes it.
  for (const k of ["7", "30", "90"]) {
    listen(el(`search-dates-k${k}`), "click", () => {
      input("search-dates-days").value = k;
      setMode("last");
      preview();
    });
  }
  listen(el("search-dates-use"), "click", () => {
    const { tokens, error } = choice();
    if (error === null) apply(tokens);
    else preview();
  });
  listen(el("search-dates-clear"), "click", () => apply([]));
  listen<KeyboardEvent>(el("search-dates-panel"), "keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation?.();
      setOpen(false);
      focusField();
    }
  });
  // Typed, recalled or cleared, the text may name a date: the button and the hint say so at once.
  listen(el("search-field"), "input", () => sync());
  sync();
  return {
    open: () => !el("search-dates-panel").hidden,
    setOpen,
    sync,
    unmount,
  };
}

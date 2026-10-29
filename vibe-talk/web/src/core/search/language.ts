/*
 * THE SEARCH LANGUAGE (`#521 server-side-search`), the page's twin of src/search_query.rs.
 *
 * Settled by the owner: Gmail's syntax as the base (implicit AND, upper-case OR, -term, "phrase",
 * parentheses) plus the shared field operators from:, in:, before:, after: and has:, over SUBSTRING
 * matching that ignores case and accents. The rules are written once, in that file's module
 * documentation, and both copies answer to the corpus in tests/fixtures/search_query.json: a change
 * to one that the other does not make fails a suite. The search box matches by them, message by
 * message (`#606 server-side-search`, in web/voice.js).
 *
 * Moved here from web/voice.js unchanged but for its types (`#624 voice-page-modularity`): pure
 * functions over strings and plain records, with no browser, clock or page state.
 */

/** A calendar day, as `before:` and `after:` name it. */
export interface SearchDay {
  readonly year: number;
  readonly month: number;
  readonly day: number;
}

/** A parsed query: a term, a field, or a combination of them. */
export type SearchNode =
  | { readonly kind: "word" | "phrase"; readonly text: string }
  | { readonly kind: "from"; readonly raw: string; readonly folded: string }
  | { readonly kind: "in"; readonly place: "thread" | "main" }
  | { readonly kind: "before" | "after"; readonly day: SearchDay }
  | { readonly kind: "has" }
  | { readonly kind: "not"; readonly child: SearchNode }
  | { readonly kind: "and" | "or"; readonly children: readonly SearchNode[] };

/** A parsed tree in the corpus's notation: ["word", "x"], ["and", ...] and so on. */
export type SearchTree = readonly (string | SearchTree | null)[];

/** A message as the matcher sees it: `text` and `author` folded once (`searchSubject`). */
export interface SearchSubject {
  readonly text: string;
  readonly author: string;
  readonly authorId: string;
  readonly createdMs: number;
  readonly inThread: boolean;
  readonly hasLink: boolean;
}

/** What `searchSubject` folds: each part of a message the haystack is made of. */
export interface SearchSubjectInput {
  readonly author?: string;
  readonly authorId?: string;
  readonly text?: string;
  readonly links?: readonly string[];
  readonly createdMs?: number;
  readonly inThread?: boolean;
}

/** Why some alternative of a query needs a scan of what the server holds, if any does. */
export type SearchScanReason = "no-term" | "not-word-aligned" | "too-many-alternatives";

/** How to ask a word-based upstream service for candidates (`searchUpstreamPlan`). */
export interface SearchUpstreamPlan {
  readonly queries: readonly (readonly string[])[];
  readonly scan: SearchScanReason | null;
}

/**
 * What lower-casing leaves for full case folding, and letters whose mark is not a combining
 * character. The same entries, in the same spelling, as `FOLD_TABLE` in src/search_query.rs, whose
 * test reads this table back and compares the two.
 */
const SEARCH_FOLD_TABLE = new Map([
  ["ß", "ss"], ["ς", "σ"], ["ſ", "s"], ["ϐ", "β"], ["ϑ", "θ"], ["ϕ", "φ"], ["ϖ", "π"], ["ϰ", "κ"],
  ["ϱ", "ρ"], ["ϵ", "ε"], ["ﬀ", "ff"], ["ﬁ", "fi"], ["ﬂ", "fl"], ["ﬃ", "ffi"], ["ﬄ", "ffl"],
  ["ﬅ", "st"], ["ﬆ", "st"], ["ı", "i"], ["ø", "o"], ["ł", "l"], ["đ", "d"], ["ħ", "h"], ["ŧ", "t"],
  ["µ", "μ"], ["ŉ", "ʼn"], ["և", "եւ"], ["ẚ", "aʾ"], ["ﬓ", "մն"], ["ﬔ", "մե"], ["ﬕ", "մի"], ["ﬖ", "վն"],
  ["ﬗ", "մխ"], ["ᲀ", "в"], ["ᲁ", "д"], ["ᲂ", "о"], ["ᲃ", "с"], ["ᲄ", "т"], ["ᲅ", "т"], ["ᲆ", "ъ"],
  ["ᲇ", "ѣ"], ["ᲈ", "ꙋ"],
]);

/** The combining diacritics the fold drops: the same blocks as `is_diacritic` in src/search_query.rs. */
const SEARCH_DIACRITICS = /[\u0300-\u036f\u1ab0-\u1aff\u1dc0-\u1dff\u20d0-\u20ff\ufe20-\ufe2f]/g;

/** Unicode White_Space, spelled out: JavaScript's `\s` is a different set (it has U+FEFF, not U+0085). */
const SEARCH_SPACE = "\t\n\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000";
const SEARCH_SPACE_RUN = new RegExp(`[${SEARCH_SPACE}]+`, "g");
const searchIsSpace = (character: string | undefined): boolean =>
  character !== undefined && SEARCH_SPACE.includes(character);

/** Fold text for comparison: diacritics dropped, lower-cased, the table applied, white space collapsed. */
export function searchFold(text: unknown): string {
  const lower = String(text).normalize("NFD").replace(SEARCH_DIACRITICS, "").normalize("NFC").toLowerCase();
  let out = "";
  for (const character of lower) out += SEARCH_FOLD_TABLE.get(character) ?? character;
  return out.replace(SEARCH_SPACE_RUN, " ");
}

/** A calendar day from `YYYY-MM-DD`, `YYYY/MM/DD` or `MM/DD/YYYY`, when it names a real date; else null. */
function searchDay(text: string): SearchDay | null {
  const separator = text.includes("-") ? "-" : "/";
  const parts = text.split(separator);
  if (parts.length !== 3 || parts.some((part) => !/^[0-9]+$/.test(part))) return null;
  let year: string;
  let month: string;
  let day: string;
  if (parts[0]!.length === 4) [year, month, day] = parts as [string, string, string];
  else if (parts[2]!.length === 4 && separator === "/") [month, day, year] = parts as [string, string, string];
  else return null;
  if (month.length > 2 || day.length > 2) return null;
  const value = { year: Number(year), month: Number(month), day: Number(day) };
  const leap = (value.year % 4 === 0 && value.year % 100 !== 0) || value.year % 400 === 0;
  const length = value.month === 2 ? (leap ? 29 : 28) : [4, 6, 9, 11].includes(value.month) ? 30 : 31;
  return value.month >= 1 && value.month <= 12 && value.day >= 1 && value.day <= length ? value : null;
}

const searchDayText = (day: SearchDay): string =>
  `${String(day.year).padStart(4, "0")}-${String(day.month).padStart(2, "0")}-${String(day.day).padStart(2, "0")}`;

/** The phrase opening at `chars[at]`: its text as typed, and the index after it. */
function searchPhraseAt(chars: readonly string[], at: number): { text: string; next: number } {
  let end = at + 1;
  while (end < chars.length && chars[end] !== "\"") end += 1;
  return { text: chars.slice(at + 1, end).join(""), next: Math.min(end + 1, chars.length) };
}

/** Folded and trimmed of spaces only (`String.prototype.trim` would also take U+FEFF, which the server keeps). */
const searchFoldedValue = (text: string): string => searchFold(text).replace(/^ +| +$/g, "");

/** Trimmed of Unicode White_Space only, as typed. */
function searchTrimSpace(text: string): string {
  const chars = Array.from(text);
  let start = 0;
  let end = chars.length;
  while (start < end && searchIsSpace(chars[start])) start += 1;
  while (end > start && searchIsSpace(chars[end - 1])) end -= 1;
  return chars.slice(start, end).join("");
}

/** The field node `raw` names when its value is valid, else null; a bare `from:` before a quote takes the phrase. */
function searchField(raw: string, chars: readonly string[], cursor: { at: number }, mayTakePhrase: boolean): SearchNode | null {
  const colon = raw.indexOf(":");
  if (colon < 0) return null;
  const name = raw.slice(0, colon).replace(/[A-Z]/g, (letter) => letter.toLowerCase());
  if (!["from", "in", "before", "after", "has"].includes(name)) return null;
  let value = raw.slice(colon + 1);
  let afterPhrase: number | null = null;
  if (value === "" && mayTakePhrase && chars[cursor.at] === "\"") {
    const phrase = searchPhraseAt(chars, cursor.at);
    value = phrase.text;
    afterPhrase = phrase.next;
  }
  let node: SearchNode | null = null;
  if (name === "from") {
    const folded = searchFoldedValue(value);
    if (folded) node = { kind: "from", raw: searchTrimSpace(value), folded };
  } else if (name === "in") {
    const place = searchFoldedValue(value);
    if (place === "thread" || place === "main") node = { kind: "in", place };
  } else if (name === "has") {
    if (searchFoldedValue(value) === "link") node = { kind: "has" };
  } else {
    const day = searchDay(value);
    if (day) node = { kind: name as "before" | "after", day };
  }
  if (node && afterPhrase !== null) cursor.at = afterPhrase;
  return node;
}

/** One token of a query: "(", ")", "-", "OR", "AND", "empty" (folded to nothing), or an atom. */
type SearchToken = "(" | ")" | "-" | "OR" | "AND" | "empty" | { readonly atom: SearchNode };

/** Split a query into tokens: "(", ")", "-", "OR", "AND", "empty" (folded to nothing), or an atom. */
function searchTokens(query: unknown): SearchToken[] {
  const chars = Array.from(String(query));
  const out: SearchToken[] = [];
  let depth = 0;
  const cursor = { at: 0 };
  while (cursor.at < chars.length) {
    const character = chars[cursor.at];
    if (searchIsSpace(character)) {
      cursor.at += 1;
    } else if (character === "\"") {
      const phrase = searchPhraseAt(chars, cursor.at);
      const text = searchFoldedValue(phrase.text);
      cursor.at = phrase.next;
      out.push(text ? { atom: { kind: "phrase", text } } : "empty");
    } else if (character === "(") {
      depth += 1;
      out.push("(");
      cursor.at += 1;
    } else if (character === ")" && depth > 0) {
      depth -= 1;
      out.push(")");
      cursor.at += 1;
    } else if (character === "-" && cursor.at + 1 < chars.length && !searchIsSpace(chars[cursor.at + 1])) {
      out.push("-");
      cursor.at += 1;
    } else {
      const start = cursor.at;
      while (cursor.at < chars.length && !searchIsSpace(chars[cursor.at]) && chars[cursor.at] !== "\"") cursor.at += 1;
      let raw = chars.slice(start, cursor.at).join("");
      let closes = 0;
      while (depth > 0 && raw.length > 1 && raw.endsWith(")")) {
        raw = raw.slice(0, -1);
        depth -= 1;
        closes += 1;
      }
      if (raw === "OR" || raw === "AND") {
        out.push(raw);
      } else {
        const field = searchField(raw, chars, cursor, closes === 0);
        if (field) out.push({ atom: field });
        else {
          const word = searchFold(raw);
          out.push(word ? { atom: { kind: "word", text: word } } : "empty");
        }
      }
      for (let n = 0; n < closes; n += 1) out.push(")");
    }
  }
  return out;
}

/** One node from several: none is null, one is itself, more an AND or OR, flattened. */
function searchCombine(items: readonly SearchNode[], kind: "and" | "or"): SearchNode | null {
  const flat: SearchNode[] = [];
  for (const item of items) {
    if (item.kind === kind) flat.push(...item.children);
    else flat.push(item);
  }
  if (flat.length === 0) return null;
  return flat.length === 1 ? flat[0]! : { kind, children: flat };
}

/** Parse a query into a tree of nodes, or null for no search. Never fails: what is not syntax is words. */
export function searchQueryParse(query: unknown): SearchNode | null {
  const tokens = searchTokens(query);
  let at = 0;
  const and = (inGroup: boolean): SearchNode | null => {
    const items: SearchNode[] = [];
    while (at < tokens.length) {
      const token = tokens[at];
      if (token === ")" && inGroup) break;
      if (token === "AND" || token === "OR" || token === ")") {
        at += 1;
        continue;
      }
      const node = or();
      if (node) items.push(node);
    }
    return searchCombine(items, "and");
  };
  const or = (): SearchNode | null => {
    const first = unary();
    const items = first ? [first] : [];
    while (tokens[at] === "OR") {
      at += 1;
      const node = unary();
      if (node) items.push(node);
    }
    return searchCombine(items, "or");
  };
  const unary = (): SearchNode | null => {
    const token = tokens[at];
    if (token === "-") {
      at += 1;
      const inner = unary();
      return inner ? { kind: "not", child: inner } : null;
    }
    if (token === "(") {
      at += 1;
      const inner = and(true);
      if (tokens[at] === ")") at += 1;
      return inner;
    }
    if (token && typeof token === "object") {
      at += 1;
      return token.atom;
    }
    // Something that folded to nothing: no term, but what a "-" before it applied to.
    if (token === "empty") at += 1;
    return null;
  };
  return and(false);
}

/** A parsed tree in the corpus's notation: ["word", "x"], ["and", ...] and so on; null for none. */
export function searchQueryTree(node: SearchNode | null): SearchTree | null {
  if (!node) return null;
  switch (node.kind) {
    case "word":
    case "phrase":
      return [node.kind, node.text];
    case "from":
      return ["from", node.folded];
    case "in":
      return ["in", node.place];
    case "before":
    case "after":
      return [node.kind, searchDayText(node.day)];
    case "has":
      return ["has", "link"];
    case "not":
      return ["not", searchQueryTree(node.child)];
    default:
      return [node.kind, ...node.children.map(searchQueryTree)];
  }
}

/**
 * Whether a message matches a parsed query (null matches everything). `subject` is
 * `{ text, author, authorId, createdMs, inThread, hasLink }` with `text` and `author` already
 * folded (`searchSubject`); `dayStartMs(day)` is the start of a day in the reader's zone.
 */
export function searchQueryMatches(node: SearchNode | null, subject: SearchSubject, dayStartMs: (day: SearchDay) => number): boolean {
  if (!node) return true;
  switch (node.kind) {
    case "word":
    case "phrase":
      return subject.text.includes(node.text);
    case "from":
      return subject.authorId === node.raw || subject.author.includes(node.folded);
    case "in":
      return node.place === "thread" ? subject.inThread : !subject.inThread;
    case "before":
      return subject.createdMs < dayStartMs(node.day);
    case "after":
      return subject.createdMs >= dayStartMs(node.day);
    case "has":
      return subject.hasLink;
    case "not":
      return !searchQueryMatches(node.child, subject, dayStartMs);
    case "and":
      return node.children.every((child) => searchQueryMatches(child, subject, dayStartMs));
    default:
      return node.children.some((child) => searchQueryMatches(child, subject, dayStartMs));
  }
}

/** A message, folded once for matching: the haystack is the author, the drawn text and each link address, one a line. */
export function searchSubject({ author = "", authorId = "", text = "", links = [], createdMs = 0, inThread = false }: SearchSubjectInput): SearchSubject {
  return {
    text: searchFold([author, text, ...links].join("\n")),
    author: searchFold(author),
    authorId,
    createdMs,
    inThread,
    hasLink: links.length > 0,
  };
}

/** The words and phrases a match may hold, folded, to mark: not those under a minus, nor fields. */
export function searchPositiveTerms(node: SearchNode | null): string[] {
  const out: string[] = [];
  const walk = (at: SearchNode | null): void => {
    if (!at) return;
    if (at.kind === "word" || at.kind === "phrase") out.push(at.text);
    else if (at.kind === "and" || at.kind === "or") at.children.forEach(walk);
  };
  walk(node);
  return [...new Set(out)];
}

/** The most upstream queries one search sends; the same number as src/search_query.rs. */
const SEARCH_MAX_UPSTREAM_QUERIES = 4;

/** Whether a folded term can be sent to a word-based upstream search: three letters or digits or more, nothing else but spaces. */
const searchWordAligned = (term: string): boolean =>
  Array.from(term).filter((character) => character !== " ").length >= 3 && term.split(" ").every((word) => /^[\p{Alphabetic}\p{N}]+$/u.test(word));

/**
 * How to ask a word-based upstream service for CANDIDATES: `{ queries, scan }`, where `scan` names
 * why some alternative needs a scan of what the server holds ("no-term", "not-word-aligned",
 * "too-many-alternatives") or is null. See `upstream_plan` in src/search_query.rs.
 */
export function searchUpstreamPlan(node: SearchNode | null): SearchUpstreamPlan {
  if (!node) return { queries: [], scan: "no-term" };
  const dnf = (n: SearchNode): string[][] | null => {
    if (n.kind === "word" || n.kind === "phrase") return [[n.text]];
    if (n.kind === "or") {
      const out: string[][] = [];
      for (const child of n.children) {
        const alternatives = dnf(child);
        if (!alternatives) return null;
        out.push(...alternatives);
        if (out.length > SEARCH_MAX_UPSTREAM_QUERIES) return null;
      }
      return out;
    }
    if (n.kind === "and") {
      let out: string[][] = [[]];
      for (const child of n.children) {
        const alternatives = dnf(child);
        if (!alternatives) return null;
        const next: string[][] = [];
        for (const left of out) for (const right of alternatives) next.push([...left, ...right]);
        if (next.length > SEARCH_MAX_UPSTREAM_QUERIES) return null;
        out = next;
      }
      return out;
    }
    return [[]];
  };
  const alternatives = dnf(node);
  if (!alternatives) return { queries: [], scan: "too-many-alternatives" };
  const queries: string[][] = [];
  let scan: SearchScanReason | null = null;
  for (const terms of alternatives) {
    if (terms.length === 0) {
      scan = scan ?? "no-term";
      continue;
    }
    const sendable = terms.filter(searchWordAligned);
    if (sendable.length === 0) scan = scan ?? "not-word-aligned";
    else if (!queries.some((query) => query.join("\u0000") === sendable.join("\u0000"))) queries.push(sendable);
  }
  return { queries, scan };
}

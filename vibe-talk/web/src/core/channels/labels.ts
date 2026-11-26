// Label names and the searches that name them (`#2279 vibe-talk-post-facto-labels`, owner
// requests 380 and 382). ONE label set of names: a hashtag label's name begins with `#` (`#kvm`,
// served as the bare slug in `Message.labels`, `#1825`), every other label is a plain name
// (`merge queue`, served in `Message.plain_labels`). The built-in Pinned label is `pinned`
// (`#1853`). Every chip, row and link the page makes searches with an EXPLICIT form —
// `label:#kvm`, `label:"merge queue"` — so the one overlap the bare form keeps (a bare
// `label:kvm` matches `#kvm` and a plain `kvm`) only touches old links and typed queries.

/**
 * A preview of the search fold for a label name (`fold` in src/search_query.rs): diacritics
 * dropped, lower-cased, white space collapsed. The server's fold is the one that decides; the
 * page only previews a name and compares names the server already folded.
 */
function foldName(text: string): string {
  return text.normalize("NFD").replace(/[\u0300-\u036f]/g, "").normalize("NFC").toLowerCase().replace(/\s+/g, " ");
}

/**
 * The server normalizer's bounds (`MAX_PLAIN_LABEL_CHARS`, `MAX_HASHTAG_SLUG_CHARS` in
 * src/labels.rs), mirrored for the picker's preview: the server decides.
 */
export const LABEL_LIMITS = Object.freeze({ plain: 40, slug: 64 });

/** The explicit search for one label name: `label:#kvm`, `label:"merge queue"`, `label:pinned`. */
export function labelQueryFor(name: string): string {
  if (name === "pinned") return "label:pinned";
  if (name.startsWith("#")) return `label:${name}`;
  return `label:"${name}"`;
}

/**
 * The label NAME a query of exactly one label term names, else null: `label:#kvm` and the older
 * bare `label:kvm` name `#kvm`, `label:"merge queue"` names `merge queue`, `label:pinned` the
 * built-in. What the Labels drop-down's closed button names.
 */
export function loneLabelName(query: string): string | null {
  const text = query.trim();
  const slug = /^label:(#?)([a-z][a-z0-9_-]*)$/i.exec(text);
  if (slug) {
    const folded = slug[2]!.toLowerCase();
    return folded === "pinned" && slug[1] === "" ? "pinned" : `#${folded}`;
  }
  const quoted = /^label:"([^"]+)"$/i.exec(text);
  if (quoted) {
    const folded = foldName(quoted[1]!).trim();
    if (!folded) return null;
    // A quoted `#kvm` is still the hashtag label, as the search language reads it.
    return folded === "pinned" ? "pinned" : folded;
  }
  return null;
}

/** What the page's preview of the server's normalizer makes of a typed name. */
export type LabelNameCheck = { readonly name: string; readonly error?: undefined } | { readonly name?: undefined; readonly error: string };

/**
 * The page's preview of the server's normalizer (`LabelName::parse` in src/labels.rs), so the
 * picker can say "Add new label #kvm" or why a name cannot be one before anything is sent. The
 * server decides; this only previews.
 */
export function checkLabelName(raw: string): LabelNameCheck {
  const trimmed = raw.trim();
  if (trimmed.startsWith("#")) {
    const rest = trimmed.slice(1);
    if (!/^[A-Za-z][A-Za-z0-9_-]*$/.test(rest)) return { error: "a # label is one word: a letter, then letters, digits, - or _" };
    if (rest.length > LABEL_LIMITS.slug) return { error: `a # label is at most ${LABEL_LIMITS.slug} characters` };
    const slug = rest.toLowerCase();
    if (slug === "pinned") return { error: "Pinned is built in: use the Pinned row" };
    return { name: `#${slug}` };
  }
  const folded = foldName(trimmed).trim();
  if (!folded) return { error: "a label needs a name" };
  if ([...folded].length > LABEL_LIMITS.plain) return { error: `a label is at most ${LABEL_LIMITS.plain} characters` };
  if (/[":\u0000-\u001f\u007f]/.test(folded)) return { error: "a label cannot hold a quote, a colon or a control character" };
  if (folded === "pinned") return { error: "Pinned is built in: use the Pinned row" };
  return { name: folded };
}

/** The label fields a served message carries. */
export interface LabelledMessage {
  readonly labels?: readonly string[];
  readonly plain_labels?: readonly string[];
  readonly manual_labels?: readonly string[];
}

/** A message's labels by name, hashtag labels first (`#kvm`), then plain ones. */
export function labelNamesOf(message: LabelledMessage): string[] {
  return [
    ...(message.labels || []).map((slug) => `#${slug}`),
    ...(message.plain_labels || []),
  ];
}

/** The label state a `labels` live event, a label write's answer or the channel list carries. */
export interface LabelState {
  readonly labels: readonly string[];
  readonly plain_labels: readonly string[];
  readonly manual_labels: readonly string[];
  readonly derived: boolean;
}

/**
 * `message` with its label fields set from `state`, as a NEW object (the store and the search
 * subjects key on identity). A state the server could not derive from its own copy of the text
 * (`derived: false`) names only the menu's hashtag labels: the typed ones this copy already shows
 * stand beside them — every held slug but the ones the menu had added.
 */
export function withLabelState<M extends LabelledMessage>(message: M, state: LabelState): M {
  let labels = [...state.labels];
  if (!state.derived) {
    const oldManual = new Set((message.manual_labels || []).filter((name) => name.startsWith("#")).map((name) => name.slice(1)));
    const typed = (message.labels || []).filter((slug) => !oldManual.has(slug));
    labels = [...typed, ...labels.filter((slug) => !typed.includes(slug))];
  }
  const next = { ...message, labels, plain_labels: [...state.plain_labels], manual_labels: [...state.manual_labels] } as unknown as Record<string, unknown>;
  // Absent when empty, as the server serves them, so an unchanged copy compares equal.
  for (const field of ["labels", "plain_labels", "manual_labels"]) {
    if ((next[field] as unknown[]).length === 0) delete next[field];
  }
  return next as unknown as M;
}

/**
 * `message` with its menu labels taken away: what a held copy becomes when the channel's
 * complete list no longer names it (they were removed on another device while this page slept).
 */
export function withoutMenuLabels<M extends LabelledMessage>(message: M): M {
  if (!(message.manual_labels || []).length && !(message.plain_labels || []).length) return message;
  return withLabelState(message, { labels: [], plain_labels: [], manual_labels: [], derived: false });
}

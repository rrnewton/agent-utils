// Discuss scope binding (`#1395 discuss-scope-binding`): the one place the
// pane-goes-with-its-view rules live, each named, so tuning a rule is one edit
// here. Pure decisions only — no DOM, no fetch — so the page tests drive every
// transition directly. The view (index.ts) holds the bound scope and acts on
// these answers; the panel's single tap router (panel.ts `answerTap`) asks
// `linkDecision` where a tap goes.

/** A scope as the wire names it: a thread, or the channel in `view`. */
export interface RawDiscussScope {
  channelId: string;
  threadId: string | null;
  view: "flat" | "main";
  label: string;
}

/** A scope as the binding sees it: All and Main are one global scope. */
export interface NormalizedDiscussScope {
  channelId: string;
  threadId: string | null;
  kind: "global" | "thread";
  label: string;
  view: "flat" | "main";
}

/**
 * Rule `widen-or-sideways-closes`: what ending a thread binding does. The
 * owner picked `"close"` (Slack 018): the pane closes and Back restores it.
 * The alternative the owner may pick later is `"fresh"` (start fresh for the
 * new scope); changing this one constant retunes the rule.
 */
export const WIDEN_OR_SIDEWAYS_ACTION = "close" as const;

/** One named binding rule: what happens, in the table below. */
export interface DiscussBindingRule {
  name: string;
  action: string;
}

/**
 * Every Discuss binding rule, named. `viewChangeDecision` and `linkDecision`
 * answer with these names so a test (and a log line) says which rule fired.
 */
export const DISCUSS_BINDING_RULES: readonly DiscussBindingRule[] = Object.freeze([
  { name: "bound-to-open-scope", action: "bind on open to the scope then on screen" },
  { name: "global-unifies-all-and-main", action: "All (flat) and Main (main) are one global scope" },
  { name: "narrowing-stays-open", action: "global -> thread keeps the panel open, bound and labelled global" },
  { name: "in-thread-link-jumps-in-place", action: "a citation inside the current thread jumps, panel open" },
  { name: "out-of-thread-link-returns-to-bound", action: "a citation outside the current thread but inside the bound global returns to the bound global view and jumps, panel open" },
  { name: "widen-or-sideways-closes", action: WIDEN_OR_SIDEWAYS_ACTION },
  { name: "fallback-closes-and-jumps", action: "a citation outside the bound scope closes Discuss and jumps to Thread or Main, Back returns" },
  { name: "out-of-window-cite-links-out", action: "a cache-resolvable citation outside the window links out-of-scope; its tap follows fallback-closes-and-jumps" },
]);

/** Normalize a raw scope: a thread stands alone; the channel is global. */
export function normalizeDiscussScope(scope: RawDiscussScope): NormalizedDiscussScope {
  const threadId = scope.threadId ? String(scope.threadId) : null;
  return {
    channelId: String(scope.channelId ?? ""),
    threadId,
    kind: threadId === null ? "global" : "thread",
    label: scope.label,
    view: scope.view === "main" ? "main" : "flat",
  };
}

/** Identity of a bound scope: channel plus global, or channel plus thread. */
export function discussScopeKey(
  scope: RawDiscussScope | NormalizedDiscussScope,
): string {
  const normalized = "kind" in scope ? scope : normalizeDiscussScope(scope);
  return normalized.kind === "global"
    ? `${normalized.channelId}:global`
    : `${normalized.channelId}:thread:${normalized.threadId ?? ""}`;
}

/** Whether two scopes bind the same Discuss (All and Main count as one). */
export function isSameDiscussScope(
  a: RawDiscussScope | NormalizedDiscussScope,
  b: RawDiscussScope | NormalizedDiscussScope,
): boolean {
  return discussScopeKey(a) === discussScopeKey(b);
}

/** What a view change does to an open Discuss. */
export interface ViewChangeDecision {
  action: "stay" | "close";
  rule: string;
}

/**
 * Decide a view change for an open panel bound to `bound` while the reader
 * now looks at `current`. Same scope (including All <-> Main) and narrowing
 * stay; widening, sideways, and a channel change close (rule 5).
 */
export function viewChangeDecision(
  bound: NormalizedDiscussScope,
  current: NormalizedDiscussScope,
): ViewChangeDecision {
  if (bound.channelId !== current.channelId) {
    return { action: "close", rule: "widen-or-sideways-closes" };
  }
  if (isSameDiscussScope(bound, current)) {
    return { action: "stay", rule: "bound-to-open-scope" };
  }
  if (bound.kind === "global" && current.kind === "thread") {
    return { action: "stay", rule: "narrowing-stays-open" };
  }
  return { action: "close", rule: "widen-or-sideways-closes" };
}

/** Where a citation tap goes. */
export type DiscussLinkAction = "jump-in-place" | "return-to-bound" | "close-and-jump";

/** Decide one citation tap. */
export interface LinkDecision {
  action: DiscussLinkAction;
  rule: string;
}

/**
 * Decide a tap on a citation naming `linkThread` (null for a top-level
 * message) in `linkChannel`, marked `outOfScope` by the server when it
 * resolved from the cache outside the window. `bound` is the Discuss
 * binding; `current` is the view on screen now.
 */
export function linkDecision(input: {
  bound: NormalizedDiscussScope | null;
  current: NormalizedDiscussScope;
  linkChannel: string;
  linkThread: string | null;
  outOfScope: boolean;
}): LinkDecision {
  const { bound, current } = input;
  const linkThread = input.linkThread ? String(input.linkThread) : null;
  if (input.outOfScope) {
    return { action: "close-and-jump", rule: "out-of-window-cite-links-out" };
  }
  if (bound === null || String(input.linkChannel ?? "") !== bound.channelId) {
    return { action: "close-and-jump", rule: "fallback-closes-and-jumps" };
  }
  if (bound.kind === "global") {
    if (current.kind === "global") {
      return { action: "jump-in-place", rule: "in-thread-link-jumps-in-place" };
    }
    // Narrowed: inside the thread on screen jumps; anything else (another
    // thread, or a top-level message) returns to the bound global view.
    if (linkThread !== null && linkThread === current.threadId) {
      return { action: "jump-in-place", rule: "in-thread-link-jumps-in-place" };
    }
    return { action: "return-to-bound", rule: "out-of-thread-link-returns-to-bound" };
  }
  // Bound to one thread: only its own messages jump in place.
  if (linkThread !== null && linkThread === bound.threadId) {
    return { action: "jump-in-place", rule: "in-thread-link-jumps-in-place" };
  }
  return { action: "close-and-jump", rule: "fallback-closes-and-jumps" };
}

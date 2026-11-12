---
title: 'discuss-scope-binding: Discuss stays with the view it was opened in'
status: open
priority: 1
issue_type: feature
assignee: mu-discussbind
labels:
- vibe-talk
created_at: 2026-11-12T19:30:00+00:00
updated_at: 2026-11-12T19:30:00+00:00
---

# Description

Owner (Slack 014/017/018): a Discuss pane goes with the view it was opened in. Today `sync()` invalidates and relabels on any view change, so an answer already shown stays on screen under the new scope's label. This issue binds Discuss to its open scope (global = All + Main as one scope, or one thread), routes citation taps by that binding, and makes cache-resolvable citations outside the window link out-of-scope instead of dropping.

# Acceptance

- Rule 1: Discuss opened in global (All/Main) or in one thread is bound to that scope; Main and All are one global scope on page and server (scope and cache key agree).
- Rule 2: narrowing All/Main -> thread keeps the panel open, still bound to global, label still global, answer intact, no new model call.
- Rule 3: a citation link inside the current thread jumps in place, panel open.
- Rule 4: a link outside the current thread but inside the bound global returns to the bound global view and jumps there, panel still open.
- Rule 5 (owner pick CLOSE): widening or sideways (thread -> All/Main, thread -> another thread) closes the panel; the history entry pushed on exit carries Discuss open + scope + record key, and browser Back returns to the thread view with Discuss open showing the same summary with zero new model calls (record restore via #1345 when present, in-memory snapshot otherwise).
- Rule 6 fallback: a link outside the bound scope closes Discuss and jumps to Thread view (in-thread message) or Main view (top-level message), Back returns.
- Slack-017: a citation the server can resolve from the cache but outside the Discuss window stays a link with its own symbol and color (in-scope keeps the curved arrow); clicking it follows rule 6. Only unresolvable (hallucinated) ids are dropped. Cache-only: zero provider calls, asserted with a fake.
- All rules live in one small table/module with each rule named; page unit tests cover every transition (narrow, in-thread link, out-of-thread link, widen, sideways, fallback with history entry, Back-restore with zero model calls); server tests cover in-window arrow / out-of-window symbol+route / dropped hallucination / no provider touch.

# Design

- New `vibe-talk/web/src/view/catchup/binding.ts`: normalized scope (global vs thread), `discussScopeKey`/`isSameDiscussScope`, the named `DISCUSS_BINDING_RULES` table, pure `viewChangeDecision`/`linkDecision`, and the one small constant for rule 5 (default and owner pick: close). `index.ts` holds `boundScope`, consults the table in `sync()`, and owns history push/restore; `panel.ts` keeps the single tap-routing function (`answerTap`) and snapshot/restore with no fetch.
- Server: `catchup.rs` shapes in-window cites with the curved arrow and cache-resolved out-of-window cites with the out marker (`scope=out` in the address, distinct symbol in the text); `http/api.rs` resolves cited ids beyond the window via the message cache only (`held_messages`), never the chat client. Symbol/color proposal is reported to the coordinator for the owner; styling keys off the address marker so it survives the sanitizer.
- Seams: mu-catchup #1345 lands first (record keyed by (channel, thread); restore reopens by scope with zero model calls); this binding replays over it and prefers that restore, falling back to the panel snapshot on main without it. mu-discussui Mod+8 reads `boundScope()`/`isSameScopeAsCurrent()` and only toggles visibility. Built on mu-discusscite #1392 anchors when landed; tap routing stays in one function until then.

# Notes

Number reserved by coordb (#1395 discuss-scope-binding). Owner picks: rule 5 = close (Slack 018); out-of-scope symbol/color to be confirmed by the owner (proposed in the landing report). Ships in the next batch after the 4:30 PM ET batch; do not take that gate slot.

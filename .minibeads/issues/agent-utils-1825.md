---
title: 'vibe-talk-hashtag-labels: typed hashtags become filterable labels with card chips, search and Settings browse'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- labels
- search
- ui
created_at: 2026-11-17T21:48:27-05:00
updated_at: 2026-11-17T21:48:27-05:00
---

# Description

[watermelon-full-preview] Owner (thread 166, Slack 6:44 PM PST 11-17, verbatim): "TODO: starting points for hash tags => labels. Ok we have a big complicated scheme for GLOSSARY and how that relates to message labeling/tagging. I haven't decided yet on many details and so it's been stalled in design. But in the meantime we could do some simple stuff with hashtags. I just wrote the following at the end of a message #kvm #benchmarking. Well whenever I do manually use hashtags. Those might as well seed the set of labels, and we can start to build the label filtering, UI support, and vibe talk. We will probably want to keep the auto-generated topics in a separate sub-namespace / prefix. I think that was in the prior proposal. But the UI stuff -- adding labels similar to current treatment of Pinned. That we can go ahead with. Adding filters on search too. We probably need a Labels (hash tags) section in the Settings to be able to browse/search them sorted by num occurrences or alphabetical. Could we "hide" a label? I dunno. TBD."

Scope confirmed with the owner by the liaison: (1) typed hashtags in message text become labels on that message under a precise token rule; (2) hashtag labels are plain, auto-generated topics later go in a separate prefix; (3) card UI styled like Pinned, click filters; (4) a label filter in search that fits the #1797 bucket chips; (5) Settings > Labels browse/search with counts sorted by occurrences or alphabetically; (6) "hide a label" is NOT built (owner TBD). Deployed on landing (owner-priority gate).

# Design

Prior proposal, read first and cited as required: `ai_docs/plans/2026-11-07-voice-overview/README.md` §8.1 (Labels: Pinned generalized) recommends ONE label namespace with an `origin: owner|machine` column and explicitly rejects separate `mine:xyz` vs `auto:xyz` namespaces as doubling filter state; §8.5 reserves `#<slug>` chip rendering for stored assignments and gives kind-prefixed storage keys (`ws:<slug>`, `th:<channel>/<id>`, `pr:<owner>/<repo>#<n>`), echoed in `ai_docs/plans/2026-11-08-shared-vocabulary/README.md` §1 (kind-prefixed snapshot keys). The 2026-11-12 Discuss glossary preamble (`ai_docs/plans/2026-11-12-discuss-glossary-preamble.md`) keeps one shared glossary store with Discuss as a consumer, never a Discuss-private path.

Adopted convention for this issue (owner direction over the §8.1 display recommendation, §8.1 storage discipline kept): a hashtag label's slug is the bare folded token (`kvm`); every label carries an origin (`hashtag` today, `machine` later) so the §8.1 split stays display-only and reversible; when auto-generated topics are built they are stored and shown under the `auto:` prefix (e.g. `auto:runner-reliability`), following the `ws:` kind-prefix precedent, and never collide with a bare hashtag slug. No auto topics are built here.

Extractor rule (server, single home; the page never re-parses): a hashtag is `#` at the start of the text or directly after whitespace or an opening bracket/quote, followed by an ASCII letter, followed by zero or more of `[a-z0-9_-]`; the label is the token lower-cased (ASCII fold) with trailing punctuation excluded by construction. Deduped per message in first-seen order. NOT a label when: inside a fenced code block or an inline code span; inside a URL (any `#` within a whitespace-delimited `http://`/`https://` run, so `https://x/#anchor` yields nothing); a Slack channel reference (`<#C…|name>` markup, i.e. `#` directly preceded by `<`); a `:thread:`/`:issue:`-style short reference (colon syntax, never matched); or `#` followed by a digit (`#123` issue numbers stay issue references). Unit-tested hard on both the Rust extractor and its page twin if one is needed.

Data: labels are DERIVED from `Message.content` on the server at serve time (decided every time like `Message.noise`, never stored, never trusted from the client), so an edit updates labels live with no migration. A pure `extract_hashtags` function is the only parser. Counts and the Settings browse read the server's message cache (the same cache server search reads) through one `GET /api/v1/labels` aggregate; no new persisted schema. If counts across history beyond the cache prove necessary, the home is a `labels` table in vibe-talk's sqlite store beside `pins` (kept-style, §8.1) — stated here before building, and only built with coordb's go.

Search (smaller design, fits #1797): the #1797 bucket chips are scope (this thread / main / threads / transcripts) and stay untouched. The label filter is a `label:<slug>` field operator in the shared search language (`vibe-talk/src/search_query.rs` + its `web/src/core/search/` twin + the `tests/fixtures/search_query.json` conformance corpus), matched against the message's derived label set — never folded into `search_text`, so a rename never rewrites history (§8.1). Clicking a card chip or a Settings row sets the search to `label:<slug>`. No new chip row.

Card UI: label chips in the row's meta line beside the Pinned chip, reusing `.pin-chip` geometry (`vibe-talk/web/styles/voice/09-to-do-view.css`); gold stays Pinned-only (§8.1: the edge budget is spent), labels use the neutral chip ink. Text is `#<slug>` so the hashtag origin reads at a glance. Clicking a chip filters to that label.

Settings > Labels: a new foldable group under Settings > Messages listing every label with its occurrence count, a filter box, and a sort toggle (most occurrences / A–Z); clicking a row opens the filtered view. "Hide a label" is an open question (owner TBD), not built.

Build order (reviewable steps): extractor + tests; server API (`Message.labels`, `label:` search, `/labels` aggregate); card UI; search wiring; Settings.

# Acceptance Criteria

- Extractor: `#kvm`, `#Benchmarking` → `benchmarking`, `#a` valid, `#1abc` and `#123` yield nothing, code spans/fences, URLs, `<#C…|…>` channel refs and `:thread:` refs yield nothing; duplicates fold to one label; labels update when a message is edited (derived, not stored). Rust unit tests pin every clause.
- Server: served messages carry their derived `labels`; `label:kvm` (and `-label:kvm`) filter in the shared search language identically on server and page (conformance corpus extended); `GET /api/v1/labels` returns each label with its count over the served cache. No new persisted schema without coordb's go.
- Card: a message's labels render as chips styled like Pinned (gold reserved for Pinned) in the meta line; clicking a chip filters the view to that label.
- Search: typing `label:kvm` filters; the #1797 bucket chips, defaults and counts are unchanged and compose with the label filter.
- Settings > Labels: all labels browsable and searchable, each with its count, sortable by occurrences or alphabetically via the toggle; clicking one opens the filtered view.
- Page tests at phone 412x915 and desktop 1280x800 for cards, filtered view, search and both Settings sort orders, in light and dark, keyboard open via a real visualViewport shrink, Discuss half-pane open in at least the phone shots; screenshot matrix (light/dark × keyboard open/closed × phone/desktop, Discuss open on phone, fixture with several hashtags) inspected image by image, folder path and one line per image in the report to coordb.
- `make validate` green on the final tree (run bare, never piped).

# Notes

Coordb amendments (2026-11-17, GO): (1) `# Heading` with a space is never a label (pinned by test); `#word` at the start of a line DOES count as a label — intended for chat. (2) A plain-text `#name` that happens to spell a channel name is still a label; only Slack's linked `<#C…|name>` form is excluded — a known, intended overlap. (3) Settings counts cover the server's message cache only and are labelled "in loaded history" in the UI; the sqlite labels table is NOT built.

Open question (owner TBD, not built): "hide a label" — whether a label can be hidden from chips/Settings without deleting it, and whether hiding is per-device or server-kept. Recorded here so the TBD is not lost; no hide control ships in this issue.

Auto-generated topics are out of scope; only the `auto:` prefix reservation above is made so they can land later without renaming hashtag labels.

## Resolution

[watermelon-full-preview] Landed on main as d1a87ebb (gate pass on 26e500f2). Hashtag extractor with hard unit tests; `Message.labels` derived at serve time (edits relabel live); `label:<slug>` in the shared search language (Rust + page twin + corpus); label groups in sqlite tables outside the migration ladder (rollback-safe), with Settings > Labels browse/search/sort, persistent checkboxes, the Combine dialog and (G) editing; card chips in Pinned geometry with hashtag-only lines hidden at render; the Entire-threads chip beside the #1797 buckets; chip taps riding the #243 place-keeper. Screenshot matrix in /tmp/mu-hashtags-shots1825 (38, inspected). Open questions stand: "hide a label" (owner TBD) and the general "rollback-compatible migrations" convention. The #1818 rect-anchor swap for chip taps remains a follow-up once its helper is on main.

Presentation (owner thread 168): a body line of ONLY hashtags is hidden at render time when its labels show as header chips (stored text, raw view and copy untouched; the line stays inline if the header has no room). Clicking a chip opens the label-filtered search with the clicked message kept at the same screen position, reusing #1818's viewport-anchoring helper (rebase onto it once landed, no second anchor). An "Entire threads" toggle stands with the #1797 bucket chips: off by default (individual hits as now); on, any thread with a hit shows whole. It ANDs with the buckets: buckets choose WHICH scopes contribute hits, Entire-threads chooses whether a hit stands alone or brings its thread.

Label groups (owner thread 167): the label-group tables live OUTSIDE the migration ladder on purpose (CREATE TABLE IF NOT EXISTS at open, `user_version` unchanged), so a binary-switch rollback to the previous vibe-talk still opens the same database — the older binary refuses any database whose `user_version` exceeds its ladder (`vibe-talk/src/store/sqlite.rs` `migrate_through`). A future ladder step that touches these tables must CREATE IF NOT EXISTS and adopt them, never assume their absence. Follow-up (general fix, not this issue): "rollback-compatible migrations" — a convention for additive steps an older binary may tolerate, so the next additive table needs no new exception.

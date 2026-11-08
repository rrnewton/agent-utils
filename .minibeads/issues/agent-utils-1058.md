---
title: 'pr-name-enrichment: vibe-talk renders GitHub PR and issue links with their names'
status: open
priority: 1
issue_type: feature
assignee: mu-prnames
created_at: 2026-11-08T12:56:33.951743014+00:00
updated_at: 2026-11-08T14:19:58.743264636+00:00
---

# Description

Owner (2026-11-08, replies 223 and 224): agent updates often list bare PR numbers. He has no time to click through opaque numbers; PRs should be referred to by name, and uniformly naming them must be deterministic infrastructure rather than something agents are expected to remember. vibe-talk already abbreviates a bare GitHub address to REPO#N (#227 github-link-abbrev, vibe-talk/src/link_labels.rs). P1 of this issue adds the NAME: the server renders an enrichable GitHub PR/issue/discussion link as REPO#N plus its name, from a machine-readable name cache on the server. P2 (separate landing, design first) carries the same enrichment to the agentctl chat bridge and agent-teams mirror posts over a shared cache format.

# Design

P1, all server-side Rust; web/src/legacy/voice.js untouched (#624 voice-page modularity in progress elsewhere).
- Label rule (vibe-talk/src/link_labels.rs, used by src/render.rs label_link): enrich a GitHub pull/issues/discussions link when its visible text is non-descriptive: (a) the address itself (today rule, incl. angle and chat-service forms), or (b) a bare reference to the SAME thread the address names: N, #N, PR N, PR #N, issue N, REPO#N, OWNER/REPO#N. The number must equal the address number and a written repo must match the address repo, else the text is left alone. Any other word in the text makes it descriptive and it is left exactly as written. New label: REPO#N NAME, keeping link target and putting the full address in title; thread suffixes (comment, review, files, ...) stay after the name. With no name cached yet the label is plain REPO#N, as today. Plain-text #N with no link is out of scope (repo ambiguous); P2 notes propose a per-channel default repo.
- Name cache (new vibe-talk/src/pr_names.rs): keyed OWNER/REPO#N. Value: curated short slug when configured (wins), else the GitHub title shortened by a stated rule (single line, whitespace collapsed, control chars stripped, cut at a word boundary at 48 chars with an ellipsis). Bounded (4096 entries, oldest-fetched evicted), persisted as one JSON file beside the store (pr-names-cache.json, next to storage.path, like documents-index.json); in-memory only when there is no store. TTL: open 24h, closed/merged 30d; 404 cached 24h; transient failure cached 5min. Optional curated names file (config path, JSON map) is the clean seam for a future glossary.
- Fetching never on the render path: render does a memory lookup only; a miss records the thread in a bounded wanted set. A background task drains it (batch of 20, 100ms apart, backs off on rate-limit answers), calling GET /repos/OWNER/REPO/issues/N through the documents GithubFetcher (same github_token_file/github_token_command token and https_proxy, api.github.com base, 401-retry). Rendered HTML is produced per read, and the in-memory render cache key gains the name-cache generation, so a name shows on the next read after it is fetched; the on-screen copy is not patched live. The search index path renders WITHOUT names (its text stays a function of the source; the bare-reference labels are address-derived, so SearchText::VERSION bumps once). Spoken form (src/speakable.rs via link_labels spoken_label) appends the name when cached.
- Config: [render] pr_names (bool, default on) disables enrichment and fetching wholesale; pr_names_file names the curated file. Off-network safety needs no config: unreachable GitHub renders today labels.
- Tests: label heuristic and named-label unit tests; cache bound/TTL/negative/persistence unit tests with an explicit clock; an offline render test proving render performs no fetch and records wanted instead; a render-level test that a bare PR URL and a #N link show the name and descriptive text is untouched; spoken-form test.

Coordinator conditions on approval (2026-11-08): (1) the fetcher is rate-limited by a token bucket, about 1 request/s sustained with a small burst and a hard 600/hour ceiling, one log line per throttle episode; the wanted set is bounded and newest-first so what is on screen wins over old history. (2) Only real page reads queue fetches: the search-index path, the post-bump reindex, summaries, speakable/read-aloud, and startup/backfill renders never add to the wanted set; a test renders the whole index corpus and asserts wanted stays empty. (3) The landing states what the render-5 reindex costs on the real devbig014 store (message count, time, background or not). (4) Titles are untrusted: escaped as text, never in attributes, bidi/control characters stripped, with a malicious-title test.

# Acceptance Criteria

- A message with a bare PR URL, an autolink, and links written #N, PR N and REPO#N renders each as REPO#N plus the cached name, same href, full address in title; a descriptively named link is byte-identical to before.
- Rendering with GitHub unreachable (empty cache) returns today labels, makes no network call, and never errors; the name appears on a later read once fetched.
- Cache unit tests pin the entry bound, open/closed TTLs, 404 and transient negative caching, curated-over-title precedence, and persistence round-trip incl. a corrupt file tolerated.
- Spoken form says the name for a cached thread and is unchanged otherwise.
- vibe-talk tests (cargo test) green; full repo gate green on the landing tree; SearchText::VERSION bumped with its corpus test updated.
- P2 design sent to the coordinator before any rs/agentctl change.

# Notes

Owner ask: chat replies 223 and 224 (2026-11-08), relayed by coordb with its spec. Builds on #227 github-link-abbrev (REPO#N for a bare address) and #187 speakable-ids-urls (spoken form). Deterministic enrichment is the mechanism; prompt reminders to agents are secondary. P2 (design to coordb first, separate commit): same enrichment on the agentctl chat bridge outbound Slack posts and agent-teams relay mirror posts, over one shared cache format/location (under ~/.local/state, never a synced path) that vibe-talk and agentctl both read; also propose the plain-text #N rule there (a per-channel default repo) since a bare number without a link does not name its repo. Note rs/agentctl changes trigger the exclusive cross-differential gate phase.

P2 design (approved in principle by coordb 2026-11-08, implementation HELD pending an owner decision on native-Slack reading): one shared cache file in the P1 persisted format at the XDG state path agent-utils/pr-names-cache.json with vibe-talk as the only writer (the P1 amendment adds the [render] pr_names_cache_file key that names it). Slack enrichment belongs in the rs/chat-subscription-slack helper mrkdwn Link arm behind a --pr-names-file flag, with no rs/agentctl diff and no config schema change. Relay and mirror enrichment belongs in the visible preview only, before sanitize_preview, with signed envelope bytes untouched. Each consumer duplicates the small pure module (thread parse for thread pages, bare-reference rule, name shortening, cache reader) with test vectors copied from P1; extracting a shared crate is deferred until a third consumer appears. Full recon with file and line citations was sent to coordb 2026-11-08 and is summarized here so it survives; the hold reason is that the owner reads Slack mostly through vibe-talk, where P1 already enriches at render time, so P2 only helps native-Slack reading while costing an agentctl-component gate plus the exclusive cross-differentials.

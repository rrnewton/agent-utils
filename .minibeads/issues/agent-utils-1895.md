---
title: 'vibe-talk-link-search-semantics: with Links on, match the query inside link addresses and names, and list links with no query'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- search
created_at: 2026-11-19T00:34:00-05:00
updated_at: 2026-11-19T00:34:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 217, 2026-11-18 9:18 PM PT): searching for "intern" after clicking the URL (Links) button, in the thread rooted in a Slack message, did NOT match the message holding `[<http://interncache-all.fbcdn.net/manifold/|interncache-all.fbcdn.net/manifold/>…/ipfsmf-test-site-loader.html#…](<https://interncache-all.fbcdn.net/manifold/oss_provenance_ipfs/flat/ipfsmf-test-site-loader.html#bafy…>)` — a Slack `<url|label>` link nested inside a markdown `[label](url)` link whose address and label both contain "intern". The owner also expected that opening search and just clicking URL, with no query typed, narrows the cards in the current scope to those containing URLs (link-listing mode), the way the Links view already lists a shown row's URLs.

Current semantics (read from the code, 2026-11-19): the shared search language (`web/src/core/search/language.ts`, twin of `src/search_query.rs`) matches a word or phrase as a folded substring of one haystack per message — author, drawn text, and each link ADDRESS, one a line (`searchSubject` folds `links` as hrefs only). A link's NAME never enters the haystack: the page hands the matcher `messageLinks(m).map(l => l.href)` (`view/channel/searchrows.ts`), so a word that lives only in a link's label matches only by accident, when the drawn text happens to spell the label out. `linksIn` (`foundation/addresses.ts`) does extract the nested forms — the owner's shape yields the outer markdown target with the Slack form kept as its name — but the name is dropped before matching. In `applySearch`, a row under Links must both match (`said`) and still show a link after the kind filter (`shown > 0`); a thread card or root whose matching link lives in a reply has no link of its own, so it is hidden. The card filters (`view/channel/cardruns.ts` `filterSideCards`) never consult Links at all ("a card has no links of its own"), so Links with no query does nothing to cards, and Links with a query cannot find a card by its replies' links.

# Design

- With Links on and a query typed, a message matches when the folded query is a substring of any of its links' href OR label (name), in addition to the existing haystack match; Slack `<url|label>`, markdown `[label](url)`, and nested forms (a Slack link as a markdown label, a `<url>` markdown target) all count, via the one extractor (`linksIn` / the drawn anchors). Matching stays folded substring (case- and accent-insensitive), never whole-word.
- A thread card, root, or unpacked row under Links matches when any held message of its thread has a matching link (href or label); under Links with a query, the existing reply-hit widening keeps working and is extended to link matches.
- With Links on and NO query, link-listing mode engages at once in every list the search touches: message rows narrow to those showing a link (as today) and draw their link lists, and cards narrow to threads holding a link. No typing is needed.
- The shared search language and its Rust twin do not change: Links matching is a page view filter layered on the parsed query's positive terms, so the conformance corpus is untouched. Coordinate with mu-labelsdrop (#1894 vibe-talk-search-extra-thread-hits) on `searchrows.ts` / `cardruns.ts`: this issue owns link matching and link-listing; #1894 owns extra/thread hits defaults and its chip.

# Acceptance Criteria

- On a test instance, a thread holding the owner's exact message shape (Slack `<url|label>` inside a markdown link) is found by searching "intern" with Links on, and by a word that appears only in a link's label; the same searches with Links off behave as before.
- Opening search and clicking URL with no query narrows the current scope to link-holding rows and cards right away, with the link lists drawn, in the channel list, a thread, and the thread cards.
- A cheap unit/page test pins the matcher: href match, label-only match, nested Slack-in-markdown match, case-insensitive substring (inside a word), and no-match; it fails on the old code for the label and card cases.
- The dynamic test runs by hand against the real server (only external upstreams stubbed) with the owner's message shape in a thread, and the screenshot matrix (light/dark, keyboard open/closed, phone 412x915 and desktop 1280x800, Discuss half-pane on phone, before/after) is captured with the shared harness and inspected, with BUILD.txt naming the shot tree.

# Notes

Lane mu-linksearch on branch mu-linksearch/1895. Functions this lane expects to edit: `view/channel/searchrows.ts` (`applySearch`, the message subject's link names), `view/channel/cardruns.ts` (`filterSideCards` links handling), and tests under `vibe-talk/tests/js/`; told to mu-labelsdrop (#1894) and coordb before editing so the two lanes' search edits do not collide.

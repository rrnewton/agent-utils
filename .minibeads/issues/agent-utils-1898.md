---
title: 'vibe-talk-nested-slack-link-in-markdown-label: a Slack link nested in a markdown link label draws as raw markup'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- links
created_at: 2026-11-19T04:18:00-05:00
updated_at: 2026-11-19T08:21:29-05:00
---

# Description

[watermelon-full-preview] Found in the #1895 vibe-talk-link-search-semantics review (2026-11-19): a message whose text nests a Slack `<url|label>` link inside a markdown `[label](url)` link label is drawn with the raw Slack markup as the link text. The owner's real message shape (the fixture already seeded in `vibe-talk/scripts/screenshots.py`, the link-search scene) is `[<http://interncache-all.example.test/manifold/|interncache-all.example.test/manifold/>…/ipfsmf-test-site-loader.html#…](<https://interncache-all.example.test/manifold/flat/ipfsmf-test-site-loader.html#bafy…>)`. The page draws the outer markdown link, but its visible label is `<http://interncache-all…|interncache-all…>…` — a leading `<` and the `|label` separator are visible on desktop. The visible text should read `interncache-all.example.test/manifold/…/ipfsmf-test-site-loader.html#…`.

# Design

- Inside a markdown link's label, unwrap a nested Slack `<url|label>` to its label text and a nested `<url>` to the url, so only plain label text is drawn. The outer href stays the markdown target with its own `<...>` angle brackets stripped, exactly as today.
- Apply the same unwrapping wherever label text is consumed as prose: the reader/read-aloud text path (no raw markup spoken), the Discuss/summary quote path, and the thread strip.
- Search and link listing (#1895) keep matching "intern" in both address and label; the extraction the matcher uses is unchanged.

# Acceptance Criteria

- On a test instance, the owner's exact nested shape draws with the plain label text `interncache-all.example.test/manifold/…/ipfsmf-test-site-loader.html#…` as the link text and the markdown target as its href, at phone and desktop sizes in light and dark themes.
- JS unit tests pin the renderer: nested `<url|label>`, nested `<url>`, plain markdown links, and plain Slack links (unchanged); the nested cases fail on the old code.
- Read-aloud text, Discuss/summary quotes, and the thread strip show the same plain label text with no raw `<url|label>` markup.
- Dynamic evidence by hand against the real server (only external upstreams stubbed), with before (main 8d190338) and after screenshots of the link-search scene from the shared harness, BUILD.txt naming the shot tree, every image inspected.

# Notes

Lane mu-nestedlink on branch mu-nestedlink/1898. Follows #1895 (mu-linksearch), which made Links search match link names; this issue is the drawing of the same nested shape. Functions expected to change: the shared link/text rendering in `vibe-talk/web/src/` and tests under `vibe-talk/tests/js/`.

## Resolution

[watermelon-full-preview] Landed on main as 7b9f8dbb + 952d59d3 (tip 952d59d3). A new `unwrap_links_in_labels` in `vibe-talk/src/slack/mrkdwn.rs` replaces, inside a markdown link's label only, a nested `<url|label>` with its label, a nested `<url>` with the url, and a nested markdown link with its label; escaped brackets and code stay literal. It runs in the Slack reader (`to_markdown`), in the renderer's prepare step for every dialect, in speech (`for_speech_with`), and in the Discuss context quote, so the owner's shape draws, speaks, and quotes as one link to the markdown target. The page's `linksIn` names the outer link by the same unwrapped words (`unwrappedLabel` in `vibe-talk/web/src/foundation/addresses.ts`), keeping #1895 Links search matching "intern" in address and label.

Evidence: Rust tests in `slack::mrkdwn`, `render` (all three dialects), and `speakable`; full lib suite 1986 passed. JS tests in `tests/js/foundation/index.test.mjs` plus page search/links suites; typecheck clean. Dynamic: shared screenshot harness against the real throwaway server (fake upstream and model stub only), scene 80-link-search-intern plus Discuss scenes 73–76, light/dark at 412x915 and 1280x800, before (main 8d190338) in /tmp/mu-nestedlink-before and after in /tmp/mu-nestedlink-after, every scene-80 image inspected — before shows the raw `<http://interncache-all…|interncache-all…`, after shows `interncache-all.example.test/manifold/…/ipfsmf-test-site-loader.html#…`. Gate on 6b8d81e0 green (validate OK); the one earlier red, `vibe-talk.check.thread-reader-follow-browser`, fails identically on pristine main and was disabled in #1903 (f404b565), which this change rebased onto before gating.

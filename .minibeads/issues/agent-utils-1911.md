---
title: 'vibe-talk-discuss-message-link-opens-new-window: Discuss message links open a new window instead of jumping in the same window'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- discuss
- ui
- links
created_at: 2026-11-19T14:07:31-05:00
updated_at: 2026-11-19T14:07:31-05:00
---

# Description

[watermelon-full-preview] Owner regression, thread 221, 2026-11-19 2:02 PM ET (verbatim but for the consumer name, redacted per repository policy): "WHOA SOMETHING BROKE!! I am trying to click the message links inside the Discuss pane and they used to jump to the message INSIDE THE SAME window... now when I click the :leftwards_arrow_with_hook: link below in 014 [consumer-a] it consistently opens a NEW vibe talk window!?! > Upshot: with the original build date as clock, [consumer-a] makes previously non-reproducing packages byte-identical; remaining blockers are three named [consumer-a] gaps. (09:12 :leftwards_arrow_with_hook:)"

Addendum, same thread: "That's a horrible ux because it takes foroever to lad and then don't even consistently see the 'Linked Message' on screen... ugh."

So the (HH:MM ↩) citation link in a Discuss summary — it points at a channel message by time — now opens a NEW vibe-talk window instead of jumping to the message in the same window. "Open links to messages here, in this app" is the setting that normally intercepts these. The fix must (a) keep the jump in the SAME window, (b) be fast, and (c) reliably land with the linked message VISIBLE and marked ("Linked message"). Prime suspects, all deployed today (main is the deployed build): 7b9f8dbb "unwrap a Slack link nested in a markdown label" (#1898 vibe-talk-nested-slack-link-in-markdown-label); 2c61217e (#1886 vibe-talk-transcripts-view, cache-only older reads); 151959aa (#1908 vibe-talk-documents-back-and-filter-place); de9931f9/e9d687c3 (#1901 vibe-talk-search-late-hits-shift-anchor); 0350ba75 (#1906 vibe-talk-agent-replies-to-owner-root-not-surfaced). Liaison hint: when the target (an older 09:12 message) is OUTSIDE the loaded or windowed range, the click may fall through to the plain link href (new window) instead of loading around the target in-app; windowed lists (#1846) and the cache-only older reads (2c61217e) are prime suspects for that path. The test fixture must include a target outside the loaded window.

# Design

- A Discuss citation tap for a message in a channel this app reads always jumps in the same window: no new window, no target=_blank navigation, no window.open; the address changes in-app and Back undoes the jump as before. This holds when the target is inside the loaded range and when it is outside it: an out-of-window target is loaded around (the seek walks to it) in-app, never handed to the browser as a plain href.
- The jump is fast and lands with the linked message visible on screen and marked "Linked message" (the #890 voice-page-linked-highlight mark), at phone and desktop sizes.
- Fix forward minimally in the owning layer (Discuss tap routing, the in-app link jump/seek, or the link rendering, whichever the bisect convicts); if the fix is non-trivial, name the commit to revert instead. No new direct scroll writer outside the scroll owner; condition-based waits only (owner rule 346).
- A committed fast browser check (next free dag.d number) asserts the same-window jump for a Discuss citation whose target is outside the loaded window: click, no popup/new page, linked message visible and marked. Dynamic testing on a real server through the real router (owner thread 185) and the screenshot matrix on a test instance (light and dark, keyboard really shrunk open and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus before/after of the targeted state), from a fresh build rebased onto current github/main with BUILD.txt in every evidence folder, using the shared screenshot harness.

# Acceptance Criteria

- On a test instance, a Discuss summary with a ↩ message link whose target is outside the loaded window: before the fix the click opens a new window (or fails to land visibly); after the fix the click navigates in the same window, quickly, with the target message on screen and marked "Linked message", and Back returns to the Discuss reading place. No target=_blank navigation and no window.open on this path.
- The same holds for an in-window citation, at phone and desktop sizes; existing Discuss and in-app-link checks stay green.
- Selected validation passes locally before the owner-tier gate; the gate passes or pre-existing reds are proven on a clean github/main checkout.

# Notes

[watermelon-full-preview] mu-discusslink, owner thread 221, 2026-11-19. Number pre-assigned by coordb; filed by mu-discusslink before implementation. Related: #543 in-app-message-links, #890 voice-page-linked-highlight, #1395 discuss-scope-binding, #1846 windowed lists, #1886 vibe-talk-transcripts-view, #1898 vibe-talk-nested-slack-link-in-markdown-label.

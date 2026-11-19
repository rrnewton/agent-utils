---
title: 'vibe-talk-discuss-message-link-opens-new-window: Discuss message links open a new window instead of jumping in the same window'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- discuss
- ui
- links
created_at: 2026-11-19T14:07:31-05:00
updated_at: 2026-11-19T17:45:35-05:00
closed_at: 2026-11-19T17:45:35-05:00
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

## Resolution

[watermelon-full-preview] Root cause, two compounding defects. (1) Every server-rendered anchor carries `target="_blank"` (the renderer stamps all links), and citations drawn in the Discuss timeline — earlier summaries and follow-up turns under a saved summary — had no tap routing at all, so those taps followed the plain in-app address into a new vibe-talk window; the answer box was routed, the timeline was not. (2) Since 2c61217e (#1886), a step back the server's copy cannot prove is answered with an empty page at the same cursor while the background fill runs, and the message seek counted those as pages: five stalls spent its whole budget, so an older linked message was given up on (reproduced at HEAD: a target 350 messages back ended "Further back than 5 pages — scroll up." after 15 s with no landing or mark; a near target landed in 0.2 s). 7b9f8dbb (#1898) was cleared: its unwrap only rewrites Markdown links with web targets, and Discuss citations are relative in-app addresses.

Fix, two landings. 9023b5e4: timeline citation taps route through the same answerTap jump as the answer's, and in-app citation anchors the panel draws lose the `target="_blank"` stamp (external links keep it), so even an unrouted tap follows the address in the same window as a deep link. 7134da49: the seek counts only steps that move the cursor and waits out a bounded number of no-progress (server-copy) steps at the same cursor, stepping again at once since each step is itself a provider-paced read.

Proof: two new fake-DOM unit tests (timeline tap jumps in-app; a drawn in-app citation loses `_blank`, an external link keeps it) fail before and pass after; a new real-server browser check (dag.d 167, `discuss_link_browser.py`, 412x915 and 1280x800) clicks answer and timeline citations to targets outside the loaded window — the timeline walk first meeting six same-cursor stalls — asserting no new page, no `_blank`, and a visible "Linked message" landing; it fails pre-fix on the stamps and on the stall walk. Evidence from landed 9023b5e4 on a local test server (never the live instance): /tmp/mu-discusslink-evidence with BUILD.txt and phone/desk x light/dark before/after shots. Gates: commit 1 local validate green, owner gate's only red (discuss-browser waits under three concurrent validates) passed its solo rerun 4/4 locally and exclusively; commit 2's gate reds were real and fixed (constants-table entry for REPLY_JUMP_STALLS; no page-clock wait in the stall retry), slices 0/4 (642) and 3/4 (641) green, and its only remaining red (screenshot scene 59, a known failure on main, no Discuss scene) passed its solo rerun, with each moved combined tree verified before landing. Unverified: behaviour on the live instance after deploy (coordb deploys and watches), and the screenshot harness's keyboard-open Discuss scenes for this change (the harness skips Discuss on a cacheless server; targeted before/after shots stand instead).

---
title: 'documents-back-chain: Back reverses the last in-app navigation, through documents'
status: closed
priority: 1
issue_type: task
assignee: mu-docreader
created_at: 2026-11-08T18:02:09+00:00
updated_at: 2026-11-09T06:12:17.171574334+00:00
closed_at: 2026-11-09T06:12:17.171573944+00:00
---

# Description

Owner (thread 214, reply 235): tapping a document link in a message opens the Documents view, "which is great. But if I do the back gesture on Android, it exits the documents view. It doesn't actually go back where I came from which was right here in the channel view, looking at your message. Back should always go right back where we came from."

Today the Documents view contributes exactly one back level (`channelLevels` in `view/shell/navigation.ts` returns `["document"]` whenever the docs view is up with a document open, `closeDocument` returning to the list). Where the reader was entered from — a channel scrolled to a specific message, the documents list, or another document — is not recorded, so Back cannot return there. Message-link jumps already solve the same shape for channels (`core/navigation` link jumps with a return spot: place, column, scroll anchor), but opening a document from a message link (`wireMessageLinks`) records no jump. Related: reply 219 (Back from a document to the list) and #1073 documents-in-app-links.

# Design

- Every in-app navigation into or within Documents records where it came from and Back reverses exactly one step: channel (scrolled to the linking message) -> document -> linked document -> Back -> Back ends on the exact channel and message; list -> document -> Back ends on the list. The Android back gesture, browser Back, and the reader's in-app Back button all take the same pop path (the BackStack/history entries in `core/navigation` + `view/shell/navigation.ts`), with the reader's scroll position restored where the machinery already restores scroll.
- Generalize the link-jump record (or add a document-jump sibling with the same spot shape) so document opens from a message, from the list, and from another document each push a returnable level; nested document opens stack. Closing the reader by other means (view switch) clears the chain the way other levels clear theirs.
- Modules: `vibe-talk/web/src/view/shell/navigation.ts` (backLevels/closeBackLevel/openDocument path), `vibe-talk/web/src/core/navigation/` (jump/visits state if generalized there), `vibe-talk/web/src/view/documents/` (reader/index open-close surface only as needed). mu-layout may touch `navigation.ts` for its button/switcher work — coordinate through coordb.

# Acceptance Criteria

- Browser check on a phone width (393x852, real input/touch, real server API): channel -> tap document link -> reader -> tap in-document link -> second document -> Android back gesture (history back) -> first document -> back again -> the original channel with the original message on screen. The in-app Back button walks the same chain.
- List -> document -> Back lands on the list; repeated Back past the chain's start behaves as Back does today elsewhere (no dead end, no exit surprise).
- Existing back behaviour stays: navigation/back browser and fake-DOM suites green, menus still close first (the outline pop-up's level included when present).

# Notes

[watermelon-full-preview] mu-docreader, thread 214. Sibling: #1081 documents-relative-links (doc-to-doc links ride this chain).

## Resolution

Landed as 3db232d5 (the chain) + 6dbe1659 (the browser walk taking both steps). Gate accounting: the full gate's shared phase had six reds, each cleared solo — one real (this issue's own browser expectation, fixed forward), the rest outside the diff. Evidence-chain note: the two Rust suites' passing rerun (agentctl 2422/2422, chat-subscription-slack 178/178) ran on 3a2d854d, #1081's tree, not on 6dbe1659; rs/agentctl, rs/chat-subscription-slack, rs/Cargo.toml and Cargo.lock are byte-identical between 3a2d854d and 6dbe1659, so the result carries. Their deep-worktree failures are #1110 test-deep-path-sockets.

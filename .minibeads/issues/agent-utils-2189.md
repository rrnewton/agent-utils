---
title: 'vibe-talk-wtf-lookup-miss-diagnosis: WTF lookup misses a definition held in the selection thread'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T05:30:00-05:00
updated_at: 2026-11-24T10:00:00-05:00
---

# Description

Selecting a hyphenated term in a status post and pressing WTF? answered
"Couldn't find what it means in recent messages or fetched documents",
although the same thread held a defining message using the spaced form
of the term. The lookup searched every cached channel with the hyphenated
form only: the folded search text keeps the hyphen literal, a hyphenated
token is never split into words, and single-word fill ("stack"-like
common words) could fill the twelve-candidate cap. The request's
message id was sent by the page but never read by the server, so the
selection's own thread was never searched first, and the selection
line's own link targets were never used as join keys. The page also
showed a fixed not-found sentence that hid what was searched.

# Design

- Retrieval stays cache-only and capped at twelve candidates, split into
  reserved buckets: the selection's own thread first (resolved through
  the request's message id), then hits sharing the selection line's
  link-target tokens, then global windows ranked by definitional fit.
- Windows are hyphen, space and no-space insensitive, with cheap
  singular/plural stemming; single common words are rarity-weighted
  last. Fetched documents get the spaced form too.
- Learned definitions stay popover-only: they must not feed the Discuss
  warm-up glossary (an owner decision, guarded by a test).
- The popover keeps its source-message link; the not-found answer says
  what was searched (thread, keywords, links) so a miss is diagnosable.

# Acceptance Criteria

- A synthetic regression through the real router (only the model
  stubbed) reproduces the miss shape — hyphenated selection, spaced
  definition in the same thread, generic-word noise — and passes after
  the fix.
- A test asserts a learned entry never enters the Discuss warm-up.
- Screenshot matrix (phone and desktop, light and dark) of the popover's
  found and not-found states on a local test instance.
- No live message text, names or ids in fixtures, shots or commits.

# Notes

Owner request 339 (fast-track, owner rule 286); diagnosis approved by
the coordinator with liaison notes before the fix was coded.

## Resolution

[watermelon-full-preview] Landed on main: the fix as 78b388f1 (thread-first
retrieval with reserved thread, link-target and global buckets;
hyphen/space/nospace and singular/plural windows ranked by definitional
fit; synthetic through-router regression; scene 99-glossary-not-found in
both harness lists) and the follow-up as f5c3a877 (plain not-found
wording naming the human search forms, link keys restricted to real
link targets with a test, formatting). Covered by the fix: hyphen,
space and joined forms, singular/plural stemming, same-thread and
shared-link retrieval. Not covered: misspellings and synonyms (no fuzzy
edit-distance matching). Learned definitions stay popover-only
(DYNAMIC_IN_WARMUP off, guarded by test). The wide-walk pace flake seen
while gating is tracked separately. #2189 vibe-talk-wtf-lookup-miss-diagnosis.

---
title: 'shots-search-scenes-two-column: screenshot scenes 100 and 101 photograph one single-column state, so no still shows a search in the two-column desk'
status: open
priority: 3
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- screenshots
created_at: 2026-11-27T03:28:00-05:00
updated_at: 2026-11-27T03:28:00-05:00
---

# Description

The screenshot harness's scenes `100-search-bucket-fold` ("search in All: Main and Threads both on, chips sum to X") and `101-search-bucket-main-only` ("search with Threads off: only Main's cards stand") came out byte-identical in each theme in the `#2354` matrix (desktop and laptop-1280, light and dark, build `8da51cc3`): the same SHA-256 per theme. Both frames show the Main view, one column, with the threads chip off and no thread hits, so scene 100 does not show what its description says. Neither shows a search in the two-column desk (Main on the left, the cards on the right), so a change to two-column search behaviour (`#2354`, `#2262`, `#2346`) has no real still to judge, and the advisory design review of `#2354` had to say so.

# Design

- Make scene 100 photograph its described state: All, with Main and Threads both on, the chips counting thread hits, the cards of both buckets standing. Give it an expectation that the threads chip is on and counts at least one hit, so a scene that falls back to the single-bucket state fails instead of passing.
- Add, or turn one of these into, a two-column desk scene of a search: Main on the left with its matches, the cards on the right, the search bar and its chips, on the desk profiles; expectations that both columns are up and both show matches.
- Keep scene 101 as Threads off, and make sure its state differs from scene 100's by an expectation, not by luck.
- Follow the harness's scene rules: a new profile joins one shard, and a scene that needs the cache server skips by name in a cacheless walk (`#2242`).

# Acceptance Criteria

- Scenes 100 and 101 produce different stills in each theme, each passing expectations that pin its own state (the threads chip on in 100, off in 101).
- At least one scene shows a search in the two-column desk, light and dark, on the desk profiles, with expectations that both columns show matches.
- The screenshot self-test and the desk and matrix walks stay green; no wall-clock budget added (owner rule 346).

# Notes

Filed for `cl-flaketriage` at the coordinator's request (liaison, 2026-11-27), from the advisory design review of `#2354`'s matrix. Low priority: after `#2354` and the lane's gate queue.

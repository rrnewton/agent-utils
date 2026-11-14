---
title: 'vibe-talk-agentic-ux-qa: taxonomy of owner UI bug reports plus round-1 agent-driven UX test'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- testing
created_at: 2026-11-14T15:52:00+00:00
updated_at: 2026-11-14T16:25:00+00:00
---

# Description

[watermelon-full-preview] New vibe-talk features repeatedly land half-broken in ways scripted tests pass over (a panel whose core reader was never finished, a document reader highlighting the wrong block with no tap-to-jump), and rounds of owner manual testing are needed to surface the obvious failures. The owner asked for a testing workstream to stand beside code review: a design review plus an agent-driven manual UX session (Playwright with screenshots judged by the model) that walks real flows end to end against a declarative should-work list. Two parts: (060) run one round against the current app and produce a bug list for owner review of the process itself, including what smoke/agentic QA exists today and whether any model in use offers computer-use tooling; (061) first mine the owner's past messages in the vibe-talk message cache for UI bug reports and boil them into a short taxonomy that can steer a testing agent's prompt. Report only in this round: fixes are separate lanes after owner review.

# Design

Taxonomy from the message cache only: copy the cache read-only, query it directly (no chat-provider API calls; recap counts are not trusted because recap drops in-window thread replies), read owner-authored messages in full behind a broad UI-vocabulary filter, and keep verbatim dated examples per failure kind. Round-1 test on an isolated instance only: the deployed release binary executed read-only with a fake upstream, scratch config/store/cache, and fake tokens, so nothing can post, mark, or react in the real deployment (which posts as the owner and marks read upstream); the report must prove that isolation. Write per-area should-work checklists before walking; walk Discuss, Documents/reader, message read-aloud, composer/send, threads, settings, search, and the reload chip at phone and desktop widths; judge every screenshot, not just state probes. Each bug records steps, expected vs actual, screenshot, severity, taxonomy class, and whether an in-flight lane already covers it (#1524, #1525, #1526, #1527, #1530, #1531).

# Acceptance Criteria

- Taxonomy under ~1.5k tokens: failure kinds with test cues and 1-2 verbatim dated examples each; audit counts (messages scanned, kept, date range, estimated misses) stated.
- Round-1 bug list sorted by severity with per-bug steps, expected vs actual, screenshot path (total under 20 MB), taxonomy class, and coverage note; in-flight-covered bugs still listed and marked.
- Isolation proven in the report (fake upstream, fake tokens, scratch state); the deployed page never driven and no live service touched.
- Process write-up verifying (not assuming) today's scripted browser tests, voice smoke scope, and absence of exploratory/design-review steps; round cost and misses stated; routine proposal (per-feature checklist, design review, pre-landing UX session) with a sourced computer-use vs Playwright comparison.
- No bug fixes in this round.

# Notes

Related in-flight lanes at round-1 time: #1524 Discuss hours control, #1525 fold triangle placement, #1526 phone half-height jump target, #1527 Summarize bottom bar, #1530 Discuss summary server-voice read failure, #1531 Discuss top bar. The taxonomy and round-1 report (with screenshots) are published in the harness documentation; links are in the lane handoff rather than in this tree. Unverified this round: threads, document star/hide/fetch-since and in-reader links, standalone message-reader scroll-place behaviour, changed-setting persistence, real keyboard geometry, voice-call flows, long-list scroll behaviour, and real (unstubbed) summary output.

## Resolution

[watermelon-full-preview] Delivered 2026-11-14. Taxonomy: 7,199 cached messages (2026-10-22 to 2026-11-14), 2,319 owner-authored, 790 read in full, 166 UI bug reports kept (38 borderline present-but-wrong), all published quotes machine-verified verbatim; ten failure kinds in ~1.2k tokens; an 81-message sample of filtered-out mail (1 clear + 1 borderline miss) makes 166 a stated lower bound. Round 1: isolated deployed-binary instance (fake upstream, fake tokens, scratch state), 69 recorded checks at 390x844 and 1280x800, 59 screenshots judged — 20 bugs (2 S1, 8 S2, 10 S3) despite every scripted probe passing; headline new find is the Discuss summary rendering folded shut, and #1530 reproduced cold. Process claims verified: 99 scripted browser tests, voice smoke is protocol probes only, a screenshot harness exists with no assigned reader, no exploratory pass or design-review step found. Closed as delivered; fixes and any routine adoption are follow-up issues after owner review.

---
title: 'vibe-talk-glossary-learned-channel-scope: a learned term that only means something in one channel is scoped to that channel'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-25T09:20:00-05:00
updated_at: 2026-11-25T09:20:00-05:00
---

# Description

Found while designing #2232 vibe-talk-wtf-lookup-log-and-replay. A WTF lookup can learn a term whose meaning holds only in the channel it was pressed in, such as an ordinal row or step name ("Row N", "Phase N") that one team's status messages define. Learned glossary rows are global by term today, so the same words in another channel would show that channel-local definition as a glossary link and popover, which is wrong.

# Design

- A learned row may carry an optional channel scope (one back-compatible field; rows without it stay global). This is a persisted-format change to the learned glossary file, reported as such before landing.
- The lookup decides the scope from its evidence: a term whose defining source and every supporting candidate sit in the pressed channel, and whose shape is an ordinal or local label (a label word plus a number or letter), is saved scoped to that channel; other terms stay global.
- The terms listing and the page's term linking honour the scope: a scoped term links only in its own channel.
- Reviewed glossary entries are unaffected.

# Acceptance Criteria

- A learned channel-local term links and answers in its own channel and not in another one; an older row without the field still loads as global.
- Tests use synthetic channels and terms only.

# Notes

Filed by the lane at the coordinator's request; not part of #2232.

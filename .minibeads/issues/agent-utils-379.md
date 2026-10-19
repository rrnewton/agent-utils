---
title: 'read-aloud-sentences: inside the block being read aloud, light the sentence being said'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T19:41:59.584510395+00:00
updated_at: 2026-10-19T19:41:59.584510395+00:00
---

# Description

[opus 5.5] Second step of the owner's read-aloud plan of 2026-10-19, after #376 read-aloud-blocks: while a block is read, the sentence the voice is saying is lit inside it, so a listener glancing at the phone finds the place at once. It must not add a request per sentence: on a voice that speaks as it generates every request boundary is a pause as long as the voice's first audio.

# Acceptance Criteria

In a block of several sentences each is lit in turn as the voice reaches it, timed against that block's own audio; a block of one sentence lights only the block; a browser without the highlighting it needs reads exactly as before; nothing in the drawn message changes. Page tests with a negative control, and the Chromium walk checks the sentences of a block lit in order.

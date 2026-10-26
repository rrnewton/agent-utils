---
title: 'c-key-copy: a bare c copies what Cmd+C copies, outside a text field'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-26T15:25:07.040538076+00:00
updated_at: 2026-10-26T16:24:16.270725753+00:00
closed_at: 2026-10-26T16:24:16.270725433+00:00
---

# Description

[opus 5.5] The owner (2026-10-26): "c for copy. We have a lot of one-key keystrokes. if the text entry box is not highlighted and I press the C key, I would like that to just copy, to be even lazier than pressing Command-C."

# Acceptance Criteria

c with no modifier, the focus outside a text field: copies the text selected where there is a selection, else the highlighted message, exactly as Cmd/Ctrl+C does, with the same confirmation; with nothing to copy, a hint and nothing copied. In the keys list and Help; page tests.

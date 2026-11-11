---
title: 'floating-reply-dock: a dock icon on the floating reply box turns it back into the inline reply editor'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-11T16:09:08+00:00
updated_at: 2026-11-11T16:09:08+00:00
---

# Description

[opus 5.5] Owner request (reply 379): "When I press q it currently pops out the floating reply dialogue. That's fine but if I change my mind I want the lower left corner of it to be a dock icon that turns it back into the inline reply-after-message UI."

Related: #1034 quote-reply-key, #727 floating quick-reply, #410 inline-reply-editor.

# Design

- A dock icon at the floating reply box's lower-left corner, with a tooltip and an accessible label.
- Clicking it moves the draft into the inline reply editor under the message it answers: the text, the quote, any attachments and the post-to-main state, with focus kept and nothing lost. The floating box closes.
- Any scroll that brings the inline editor into view goes through the single scroll owner (#1251 scroll-single-owner).
- Owner fast-track: path-selected gate, deployed on landing.

# Acceptance Criteria

- A desktop browser walk: press q, type a draft with a quote, click the dock icon; the inline editor under the message holds the same draft, has focus, and the floating box is gone.
- The same walk with post-to-main ticked keeps it ticked after docking.
- No wall-clock waits in the test (owner rule 346).

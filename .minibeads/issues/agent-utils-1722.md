---
title: 'vibe-talk-mention-link-not-clickable: a mention that renders blue must be a real link'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T11:17:02-05:00
updated_at: 2026-11-16T11:17:02-05:00
---

# Description

[watermelon-full-preview] Owner report (fast-track, owner rule 286), 2026-11-16 11:13 AM ET, part of a bigger @-targeting TODO that a sibling design lane handles; this issue fixes only this bug: when a message with a working @-reference is copy-pasted, it looks like a profile link in angle-bracket pipe form, and renders blue and formatted as if special, but it is NOT a hyperlink. On Slack itself it is a hyperlink that opens the profile. The owner asks that, if possible, clicking the link opens Slack and the person's profile, if that is linkable.

In vibe-talk, a mention that renders blue/special must be a real link. The stored forms of mentions in the live cache (user mentions, profile links like the one above, and Slack rich_text user elements) must each render as a link: to the Slack profile when a profile URL is derivable (using the configured Slack workspace URL, or the profile link Slack itself gave), otherwise to the URL in the stored link. The link opens in a new tab/app and does not trigger message selection or card collapse. No popup/autocomplete work (that is the design lane).

# Acceptance Criteria

- Unit/page tests for each mention form produce one anchor with the right href and label: a profile-link form; a plain user mention with and without a known display name.
- Condition-based waits only (rule 346).
- Acceptance screenshots on a TEST instance (phone and desktop) showing a mention as a link (hover/underline state if possible); paths in the report and handoff; the liaison reviews.

---
title: 'discuss-exclude-owner-messages: the owner is context, never unread'
status: open
priority: 0
issue_type: feature
assignee: mu-discussfoot
labels:
- vibe-talk
created_at: 2026-11-12T17:00:00+00:00
updated_at: 2026-11-12T17:00:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 005 (via coordb): "Those remaining UNREAD messages should also not include messages from ME. We are summarizing what the agents say, and the messages from me are just for CONTEXT and constructing my POV for the summarizer."

Owner reply 008 clarification: no prompt instruction about mentioning owner messages; owner context messages remain valid citation targets — the (HH:MM) citation validator must not strip links to owner message ids.

# Design

- The owner's own messages (via `Owner::of`: `[owner] user_ids` plus each provider's `owner_user_id`; never hardcoded) are excluded from the unread set, the counts (including the Summarize control's N) and the footer, but still sent to the summarizer as context.
- The partition lives in `catchup::build_context`: `unread_ids`/`markable_ids`/`window.unread_count` count only news (`!read && !owner`); the prompt still carries every kept message with its `[OWNER]` label.
- `shape_answer` keeps validating citations against all sent messages, so a briefing citing an owner message keeps its link while that message never appears in the footer.

# Acceptance Criteria

- A window whose only unread messages are the owner's reports zero unread; the Summarize count excludes owner messages.
- Owner messages still appear in the summarizer prompt as context.
- A briefing citing an owner message keeps its (HH:MM) link, and the owner message does not appear in the footer. Page/Rust tests pin this; walks pass at 390x844/375x667/1280x800; gated at owner tier, landed on its own.

# Notes

Owner replies 005/008. Sibling: #1347 discuss-footer-dense-links. Parent: #1316 catchup-side-agent.

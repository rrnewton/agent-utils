---
title: 'slack-post-cap-raise: converted and mrkdwn posts split at 4,000 bytes while the plain path allows 12,000'
status: open
priority: 2
issue_type: task
labels:
- agentctl
created_at: 2026-11-16T17:12:48.000000000+00:00
updated_at: 2026-11-16T17:12:48.000000000+00:00
---

# Description

[watermelon-full-preview] Owner ask 2026-11-16 (:thread:112): long replies split into many Slack posts because `MAX_MRKDWN_BYTES = 4000` in `rs/chat-subscription-slack/src/mrkdwn.rs` caps `max_chunk_bytes` on the converted and mrkdwn paths (`rs/chat-subscription-slack/src/outbound.rs`), while the plain path already allows `SLACK_MAX_BODY_BYTES = 12_000`. Evidence gathered before any code (no posts made): the 4,000 figure came in with bcf90a2b (#530) as "the vendor `--slack-mrkdwn` byte cap", and it is real — the vendor client's send help documents `--slack-mrkdwn` as "at most 4000 bytes" against `--markdown` / `--text` at 12,000, and the vendor client's non-posting validate refuses 5,000- and 11,000-byte mrkdwn bodies with: "--slack-mrkdwn must be at most 4000 bytes. To send this body, pass it as --markdown=<body> instead: standard Markdown, up to 12000 bytes, with *bold* as **bold** and <url|label> as [label](url)." (`MetaCLIValidationException`, caller fault). Raising the constant alone would therefore make every longer post fail at the vendor.

# Design

Approved direction (coordinator, 2026-11-16): on the converted path, a body whose converted form exceeds 4,000 bytes is sent once as the original Markdown via `--markdown` (the vendor renders it; one post up to 12,000 bytes, splitting only above that), keeping local conversion and Block Kit headings at or under 4,000 bytes. The plain mrkdwn path keeps splitting, since its body is already mrkdwn and `--text` would show the markup literally. The route sits behind a bridge configuration switch, `long_markdown_route` (boolean, default off — today's splitting), documented with the other bridge config keys: the vendor's Markdown rendering is what the local conversion replaced (#530), so a deployment enables it only after checking one real long post. `MAX_SECTION_CHARS` (3,000) and `MAX_BLOCKS` (50) stay honored wherever blocks are posted. Fidelity check before code (no posts): the vendor documents `--markdown` as rendering headings, lists, tables and code fences natively (pipe tables become native tables), so nothing the owner relies on is lost; the vendor validate confirms validity and byte counts but shows no rendering.

# Acceptance Criteria

- A converted body of about 10 KB posts as one message (or, under the chosen direction, the documented equivalent), with every posted block within `MAX_SECTION_CHARS` and `MAX_BLOCKS`.
- A body over the applicable cap still splits, and no post ever exceeds the vendor's per-flag byte limit.
- The transport documentation (`TextFormat::SlackMrkdwn` "at most 4,000 bytes") matches the implemented behavior.

# Notes

Vendor evidence recorded 2026-11-16 from the vendor client's send help and two non-posting validate probes (5,000 and 11,000 bytes), which post nothing. Follows #1708 chat-links-utf8-panic / #1709 chat-links-issue-label (landed e878d2ac).

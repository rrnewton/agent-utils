---
title: 'slack-uncarried-root-recovery: deliver a thread root the event stream never carried, found from its reply'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- slack
depends_on:
  agent-utils-415: discovered-from
created_at: 2026-10-22T03:14:08.919145346+00:00
updated_at: 2026-10-22T03:14:08.919145346+00:00
---

# Description

[opus 5.5] Follow-up to #415 slack-file-share-delivery. The Slack event stream never publishes some messages: on 2026-10-21 the owner's thread root with an attached file (ts 1792637709.308909) had no event of any type in his queue, while his plain reply in the thread (seq 232688) arrived. #415 made the plugin name such a root in its diagnostics log, once, when a reply reveals it; the root itself is still never delivered, so the agent gets the reply without the message it answers.

# Design

Preferred first step (coordinator, 2026-10-21): when an admitted reply arrives whose thread root the stream never carried (the #415 detection), the plugin makes ONE bounded slack.message list call for that root (--thread-ts=<root> --oldest just below it --limit small, columns message_ts,thread_ts,author_user_id,is_author_bot,subtype,file_ids,content). If the root's author is an allowed sender and the root is not already a request, it is delivered as a request ahead of the reply, with its file names (or ids, if names are not listed) in the text. This fixes the owner's case without a polling redesign. Constraints to respect: (1) the host refuses a replayed batch whose content differs from the saved boundary (agentctl BoundaryMismatch), so the root and reply must be delivered so a replay is identical, e.g. the root as its own batch at the reply's cursor before the reply's batch, or both in one batch, decided only from listing output that a replay sees the same way; (2) listing needs an app id for a user-identity client (--as), which the inbound configuration does not carry today; (3) one client run costs about 1.5 s and 225-310 MB, acceptable for this rare path. A full history poll (to also catch a file reply in a thread, or a file root nobody replies to) is a later, separate decision.

# Acceptance Criteria

Fake-transport test: the captured reply shape after an uncarried root, a fake listing of a root with file_share and file_ids from an allowed sender: the root is delivered first with the file names in its text, then the reply; a replay after restart yields identical batches; a bot-authored or already-delivered root delivers nothing extra; a listing failure delivers the reply alone and logs it.

---
title: 'vibe-talk-agent-replies-to-owner-root-not-surfaced: agent thread replies to the owner''s own messages are not surfaced where the owner reads'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-state
created_at: 2026-11-19T07:29:00-05:00
updated_at: 2026-11-19T07:29:00-05:00
---

# Description

[watermelon-full-preview] Owner report, thread 219 (2026-11-19 7:29 AM ET): an agent team "won't respond to me on slack via vibe talk ... I can't get a response when using its pinned thread." The replies did arrive and are unread in the server cache: the owner's 06:18 top-level root got bridge replies at 06:20 and 06:41, both unread; the 07:27 root got a reply at 07:28, unread; the owner's 07:28 reply inside a pinned thread got replies at 07:29 and 07:40, unread. The owner posts top-level (or inside a pinned thread), the agent answers in a thread, and the owner sees "no response".

This follows #222 unread-replies ("N replies, M unread" on Main, roots with unread replies kept under Hide read, closed 2026-10-09) and #1881 vibe-talk-own-messages-read-on-arrival (landed, deployed 2026-11-19 3:00 AM ET: the owner's own roots are now read on arrival — a possible regression source if that read root is now treated as fully read for surfacing, hiding, and unread navigation). Search of the `mu-*` branches found no in-flight lane for this; #1316 catchup-side-agent is the Discuss panel and does not surface thread replies in Main.

# Design

- Thread replies from others to the owner's own message — a top-level root, or a message inside a pinned or other thread — surface where the owner is reading: in Main, visibly on that root's card, as a count of new replies with an inline preview of the newest reply (the clearest of the brief's options, because a bare count is easy to overlook and a preview proves an answer exists without opening the thread); tapping it opens the thread at its first unread reply, as the existing chip does.
- The card stays visible under Hide read while it has unread replies, including when the root is the owner's own and read on arrival (#1881).
- Those replies count in the unread jump (Shift+Up/Shift+Down and the jump-to-unread chip) and in the unread/newest counts: a Main row whose thread holds unread replies is not "read" for navigation.
- For the pinned-thread case, the pinned thread's entry in the thread picker/strip and its pin button show the new replies (an unread count on the pinned thread's row), so a pinned thread with waiting replies is visible without opening it.

# Acceptance Criteria

- On a real server with the shared harness, the exact owner shape is recreated (owner top-level root, then 2 agent thread replies arrive live; owner reply inside a pinned thread, then 2 agent replies): BEFORE from main shows the owner seeing nothing; AFTER shows the replies plainly on the Main card (count plus newest-reply preview), under Hide read, in the unread jump, and on the pinned thread's strip/pin entry.
- Page tests at phone and desktop sizes pin the exact defect (live thread reply to an own, read root surfacing in Main, kept under Hide read, counted by Shift+Up/jump-to-unread; pinned thread entry showing new replies), with condition-based waits only.
- Screenshot matrix per the standing owner-UI acceptance (light/dark, keyboard open really shrunk/closed, phone 412x915 and desktop 1280x800, Discuss open on phone, before/after of the targeted state) with BUILD.txt in the shot folder, via the shared harness; every image looked at.
- Dynamic evidence: the feature is exercised end-to-end through the real server router on a test instance (not the live instance) before landing, with commands, output, and screenshots attached.

# Notes

Owner-priority, fast-track: deployed on landing. Related: #222 unread-replies (the Main chip and Hide-read keeping this builds on), #1881 vibe-talk-own-messages-read-on-arrival (own roots read on arrival; check it does not hide a root with unread replies), #1316 catchup-side-agent (Discuss; not this surface). No consumer, host, or person is named here beyond the owner's own report.

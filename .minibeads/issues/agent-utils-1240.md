---
title: 'chat-file-delete: chat reply and publish remove the message file once the text is durably stored'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- chat
assignee: mu-chatdup
created_at: 2026-11-10T17:54:21+00:00
updated_at: 2026-11-10T17:54:21+00:00
---

# Description

[watermelon-full-preview] Owner request (owner reply 342, 2026-11-10 12:51 PM ET): "Yeah I think deleting the message file on ingestion is fine." `agentctl chat reply --file F` and `chat publish --file F` leave F in place after the bridge has durably stored the text, so caller scratch directories fill up: one such directory alone holds 633 files from today, and nothing but the 10-day /tmp cleaner removes them.

# Design

- `chat reply --file F` and `chat publish --file F` delete F once the text is durably stored: after the state write (reply) or the accepted post (publish), never before.
- A refused command (for example the near-duplicate refusal) or a failed one leaves the file in place.
- `--keep-file` opts out. `--file -` reads stdin (nothing to delete).
- Delete only a regular file the caller owns: refuse symlinks and anything not owned by the caller's uid. Print "removed F" on success.
- Update the chat userguide and the prompt footer text where it mentions the file.
- Rust is the primary edition; keep the Python mirror in parity for the same commands, per the repository's edition rules.

# Acceptance Criteria

- A stored reply and an accepted publish each remove their `--file` and print "removed F".
- A refused or failed command leaves the file in place.
- `--keep-file` keeps the file; `--file -` reads stdin.
- A symlink path and a file owned by another uid are refused (not deleted), with the file left in place.
- Focused tests cover each case in the Rust edition, with the Python mirror kept in parity.

# Notes

Background: a liaison reply directory held 633 files from one day (2026-11-10); only the 10-day /tmp cleaner removes them otherwise. Lands with or after #1239 chat-duplicate-refusal, whose refusal is one of the refused cases that must keep the file.

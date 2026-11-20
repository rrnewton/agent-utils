---
title: 'wrkslots-remote-ssh-match-by-url: match per-remote SSH settings by URL or host, not only remote name'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
- ssh
created_at: 2026-11-20T07:00:00-05:00
updated_at: 2026-11-20T07:00:00-05:00
---

# Description

[watermelon-full-preview] Follow-on to #1940 wrkslots-per-remote-ssh-command (coordb reservation 2026-11-20). The #1940 settings match by remote NAME: an entry for origin applies to every repository's origin, whatever host its URL points to. A consumer with several origins on different hosts needs matching by URL or host instead.

# Design

- To be designed when picked up: extend `remote_ssh_commands` matching so an entry can name a URL or host (exact URL, or host suffix) in addition to a remote name, with a documented precedence (most specific wins) and the same shlex/injection guards and doctor display as #1940.
- Reader skew to be re-checked then: any new config shape must stay ignorable by old readers via `ignorable_keys`.

# Acceptance Criteria

- Not yet scoped; see Design.

# Notes

Filed as an issue only in the #1940 lane, per coordb. No implementation here.

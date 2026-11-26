---
title: 'vibe-talk-issue-files-real-channel-id-cleanup: replace the real channel id quoted in two issue files with a synthetic one'
status: open
priority: 3
issue_type: chore
labels:
- hygiene
created_at: 2026-11-26T05:12:00-05:00
updated_at: 2026-11-26T05:12:00-05:00
---

# Description

Two issue files, `.minibeads/issues/agent-utils-650.md` and `.minibeads/issues/agent-utils-756.md`, quote a real chat channel id in their prose. This repository is public, and a real channel id has no place in it: the same id nearly went into code and tests in `#2276 vibe-talk-client-debug-mode` as a sample "looks like a Slack id" value before review caught it. This issue deliberately does not repeat the id; it is "the real channel id in those two files".

# Design

- Replace the real channel id in those two files with the repository's synthetic Slack channel id (`C0123ABCDEF`, as the README and the example configuration already use), keeping each sentence's meaning.
- A normal forward commit on main: no history rewrite and no force push. The id stays in history; the goal is that the current tree no longer carries it.
- While there, check the rest of `.minibeads/issues/` and `ai_docs/` for other real chat ids with `git grep -n -E '\b[CDGUW]0[A-Z0-9]{8,10}\b'` and list (without quoting) any further hits in this issue's Notes for the coordinator to decide on.

# Acceptance Criteria

- `git grep` for the real channel id on main finds nothing in the current tree.
- `scripts/check_issue_parse.py` and `scripts/check_client_names.py` pass on the commit.
- The commit changes only issue files (land-main as issue-only).

# Notes

Filed by cl-debugmode at the coordinator's request (2026-11-26), from the liaison's review of `#2276`. Low priority: hygiene, no behaviour change.

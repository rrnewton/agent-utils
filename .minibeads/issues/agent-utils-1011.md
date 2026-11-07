---
title: 'merge-queue-commit-tree-stdin: commit-tree passes the message as argv and fails on large messages'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-07T12:18:00.000000000+00:00
updated_at: 2026-11-07T14:18:27.000000000+00:00
closed_at: 2026-11-07T14:18:27.000000000+00:00
---

# Description

A consumer's merge queue stopped with `could not run git commit-tree:
Argument list too long (os error 7)`. `Git::commit_tree` in
`rs/merge-queue/src/git.rs` passes the commit message as an argv string
(`-m <message>`). Linux limits a single argv string to `MAX_ARG_STRLEN`
(131,072 bytes), so any commit whose message exceeds that limit — the
reported case is 138,475 bytes — cannot be replayed or merged by the
queue at all, even though `git` itself accepts messages of any size.
The failure stops the queue: the change can never land.

# Design

- Pass the message on standard input instead of argv: `git commit-tree`
  reads the message from stdin when no `-m`/`-F` is given (equivalently
  `-F -`). No message bytes travel in argv, so no per-string limit
  applies.
- Audit all of `rs/merge-queue` for other argv-passed messages or
  bodies (`commit`, `tag`, `notes`, `git commit -m`, and `gh`/`curl`
  bodies or other large strings such as PR bodies and trailers passed
  as arguments) and fix each the same way.

# Acceptance Criteria

- A commit whose message is over 131,072 bytes (for example 140,000
  bytes) replays through the real queue code path; it fails with
  `Argument list too long` before the fix and succeeds after.
- No production path in `rs/merge-queue` passes a consumer-provided
  message or body as a CLI argument.
- Existing merge-queue behaviour is otherwise unchanged.

# Notes

Filed from GitHub issue 308
(https://github.com/rrnewton/agent-utils/issues/308), reported by a
consuming project's merge queue. Consumers are referred to generically
here, per repository convention.

Landed as 98f4b65d by mu-argmax (watermelon-full-preview): `commit_tree`
feeds the message on stdin and the regenerate commit uses `-F -`; a
140,000-byte message replays cleanly (regression test
`replay_preserves_a_message_over_the_argv_limit`).

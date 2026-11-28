---
title: 'live-seal-classify-edition-disagreement: with a live actor and no wrkslots process on the sealed target, the Rust classify sometimes calls a validation-batch seal in progress where Python calls it interrupted'
status: closed
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- rust
- editions
created_at: 2026-11-28T13:08:00-05:00
updated_at: 2026-11-28T15:29:00-05:00
closed_at: 2026-11-28T15:29:00-05:00
---

# Description

`py/wrkslots/tests/test_classify_journals_rust.py::test_classify_live_seal_structured_blocking_reason_matches_python` writes a validation-batch seal whose actor is the test process itself (live), with no wrkslots process acting on the sealed target. Both editions should then call the seal interrupted: "interrupted validation-batch seal recorded", with structured `actor_state` "live".

Twice in the kept gate logs, the Rust edition said instead: "wrkslotsd: validation-batch seal in progress (pid N, since 2026-11-07T00:00:00+00:00) recorded in ... wait for it to finish and retry".
- head ecf2fab5, `python.wrkslots.shard-6`, 2026-11-28 (the console gate log of the lane that ran it);
- `python.wrkslots.shard-2` of another gate on 2026-11-28 (cl-flaketriage's console gate log for that gate).

The Python edition, in the same runs, said interrupted. So the Rust edition sometimes finds the seal's operation in progress where Python does not. Which evidence it found is not in the logs.

# Design

- Repro first:
  - run the case repeatedly in the gate's wrapper on a busy host;
  - when it fails, capture what the Rust edition counted as an acting process (its pid, command and how it touches the target).
- Then name the cause and fix it so both editions apply the same rule: what counts as "a wrkslots process acting on the sealed target". Likely candidates to check: a process match broader than the target, such as any wrkslots process by command line; or the test's own helper processes.
- A differential that pins the rule: an unrelated wrkslots process elsewhere does not make the seal in progress; one acting on the target does.
- No wall-clock budget; no new persisted field.

# Acceptance Criteria

- The cause is named from a reproduction.
- Both editions classify the same seal the same way in that situation, with a differential test.
- The case passes in its shard in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the 48-hour triage (item E, recurring).

## Resolution

Fixed in d1c1c116 (`rs/wrkslotsd/src/ensure_states.rs`), gated as c545284f: gate 1795893788680089755-7502e6da (full selection).
- In that gate the changed path was green: `rust.wrkslots.test`, the wrkslots debug test, and every `python.wrkslots` shard, including this case's own.
- Its off-path reds: `rust.agent-teams.test` and `vibe-talk.check.documents-read-browser` passed in a solo rerun on the unchanged commit (gate 1795897455310050737-8e2204de); `vibe-talk.check.read-pause-reply-browser` is known-red in an untouched component.

Cause, from a reproduction:
- With process churn inside the test's own PID namespace, the case failed 1 in 30 on the old code. The Rust classify named an exiting churn process as the seal's live operator.
- The Rust scan counted every `/proc/<pid>/cmdline` read failure except NotFound as a live wrkslots process acting on the sealed targets. A process that exits between the open and the read fails with ESRCH. The Python edition skips it as exited.

Fix:
- `cmdline_read_error_is_exit`: NotFound (ENOENT) and ESRCH mean the process exited, and it is skipped; any other failure still counts.
- A deterministic unit test covers the error kinds.
- After the fix, the churn repro passed 60 of 60, 54 of them at load1 >= 237.


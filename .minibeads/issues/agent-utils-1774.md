---
title: 'wrkslots-dangling-nested-gitfile-under-ignored: a dangling gitlink under ignored content is probed as a repository and blocks removal'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T09:50:00-05:00
updated_at: 2026-11-17T09:50:00-05:00
---

# Description

[watermelon-full-preview] A slot holds `ignored/reverie-third-party-a32b6494/`, a copy of a project's third-party submodules (dynamorio, e9patch, sabre). Each copy keeps a `.git` file whose relative gitdir resolves to nothing. Removal refuses with "cannot determine whether remote 'origin' has a push URL (exit 128): fatal: not a git repository: .../ignored/reverie-third-party-a32b6494/dynamorio/../../../../../../.git/worktrees/.../modules/...". A nested `.git` file whose gitdir does not resolve (a dangling gitlink) is not a repository. Under ignored content it should be judged like any other ignored file (archived or discarded per --archive-ignored/--discard-ignored), never probed for remotes. A consumer is working around it by hand.

# Design

- One classification in each edition: a nested `.git` that is a file with an unresolvable gitdir is a "dangling gitlink" and counts as plain files. Nested-repo discovery (the push-URL / containing-remote probe over unlisted submodules, and the ignored-file collection that recurses into a directory holding `.git`) uses it in `py/wrkslots/cli.py` and in `rs/wrkslotsd/src/remove_write.rs` / `rs/wrkslotsd/src/finish_write.rs`.
- A nested `.git` directory, or a gitfile that does resolve, keeps the current behaviour, including refusing for real unpushed work; salvage for real repositories is not weakened.
- The dangling gitlink is named in the salvage receipt/evidence by path only, in the bounded summary style #1772 is introducing. This change stays in the nested-repo discovery/classification code, not in the evidence serialisation, and rebases onto #1772 when it lands.

# Acceptance Criteria

- A dangling gitfile under `ignored/` is archived or discarded with the other ignored files and removal proceeds, in both editions with differential parity.
- A resolvable nested repository with an unpushed commit still refuses.
- A nested `.git` directory is judged exactly as before.

# Notes

Known edge, deliberately left: a gitfile whose gitdir exists but is not a repository (for example an empty directory) is still probed as a repository and refuses as before. Only a gitdir that does not resolve at all is a dangling gitlink here.

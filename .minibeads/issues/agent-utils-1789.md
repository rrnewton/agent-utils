---
title: 'wrkslots-validation-exclusion-sticky-parent-refusal: ownerless validation cleanup refuses private dirs under the validate checkout tree'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T12:56:13-05:00
updated_at: 2026-11-17T12:56:13-05:00
---

# Description

After a nightly validate run, validate-run cleanup removes the run's private Cargo home and runtime root through wrkslots ownerless validation recovery. The Cargo home is retained:

```
validate-run: private Cargo home and runtime root RETAINED: wrkslots retained the Cargo home: WARNING: 4 registered slot(s) have missing or inconsistent storage. ...
WARNING: 1 existing unregistered worktree directories remain outside wrkslots records ...
WARNING: 2 Git-registered path(s) inside the managed root have no active row ...
REFUSED: validation exclusion requires exactly one trusted root-owned, same-filesystem sticky parent; found 0
REMEDY: run 'wrkslots recover --help', correct the named condition, then rerun 'wrkslots recover' with the required arguments
```

The private directories live under the validate checkout tree, not under a root-owned sticky directory. Re-checked on current main: the refusal text and the single hard-coded exclusion root are unchanged.

# Design

Under investigation by mu-stickyparent. Findings reported to coordb before any code change: whether the consumer layout violates the documented contract or the check is wrong for directories under the validate checkout tree, what the actionable refusal must name (judged path, each candidate parent with owner/mode/sticky/filesystem, supported layout, real remedy), and the fix direction coordb chooses.

# Acceptance Criteria

- The refusal for this condition names the judged path, every candidate parent considered and why each failed (owner, mode, sticky bit, filesystem), and the supported layout, with a remedy specific to this condition rather than generic help text.
- Per coordb's decision, either private directories under the validate checkout tree are supported with an equivalent safety argument, or the rule is kept and the error and documentation state the required layout clearly.
- Deterministic regression tests cover the exact defect in both editions where the tool has both.

# Notes

Filed by mu-stickyparent; number pre-assigned by coordb. Overlaps mu-censusexit (#1769), mu-rescuetips (#1785), mu-evidencecap (#1772) and mu-schemaskew (#1788) in py/wrkslots/cli.py; keep the change local and rebase right before the gate.

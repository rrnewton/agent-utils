---
title: 'create-abort-hook-partial-submodule: abort-create forgives its own hooks partial submodule init'
status: open
priority: 1
issue_type: bug
assignee: mu-fence
labels:
- wrkslots
created_at: 2026-11-11T18:31:30+00:00
updated_at: 2026-11-11T18:31:30+00:00
---

# Description

[watermelon-full-preview] From the originating validator (read-only, evidence preserved): `wrkslots recover --slot validate-fresh-4bfde9259f7f-1857362-442ef2b6 --abort-create` refuses rc 3:

    cannot abort changed checkout checkout: source state is dirty (1 .M SCM. 160000 160000 160000 071c76f6 071c76f6 agent-utils); preserve it for inspection

The only dirt is the create hook's own half-done `git submodule update --init --recursive`, killed mid-run (the create journal holds the hook at status `running`): one submodule clone sits at its default commit, never checked out, with staged deletions; the gitlinks are unchanged; the other submodules were never initialized; nothing was authored. Every donor-job cancellation that lands in a validate slot's create hook leaves such a journal, and it blocks RETIRE-RETAINED for every later run on that donor.

# Design

- `abort-create` recognises dirt that is ONLY its own running hook's partial submodule init: every dirty submodule was uninitialized at create start, its gitlink still names the recorded commit, the dirt is the partial-init shape (clone present at the wrong commit with staged deletions, or never initialized), and there is no other dirt and no authored work anywhere in the slot.
- Anything else - a moved gitlink, dirt in an initialized submodule, ordinary file changes, untracked authored work - keeps the current refusal and preservation.
- Applies in both editions (Python `py/wrkslots/cli.py` and Rust `rs/wrkslotsd`), or ships a parity note where one edition cannot decide.
- Optional hardening, for the merge-queue side (separately owned): a guard against cancelling a donor job inside the create hook; noted here, not built here.

# Acceptance Criteria

- A fixture whose create hook is killed mid-submodule-init aborts cleanly with `recover --abort-create`; before the fix it reproduces the `source state is dirty` refusal.
- Fixtures with a moved gitlink, dirt inside an initialized submodule, or authored file changes still refuse and preserve everything.
- Both editions agree, or the parity gap is recorded here with the reason.
- The gate passes at normal priority and the fix lands on `main`.

# Notes

Assigned by the coordinator from the devbig014 ledger block (1330-1399). Evidence preserved in place on its host: nobody touches that tree or checkout. Normal priority; no owner tier needed.

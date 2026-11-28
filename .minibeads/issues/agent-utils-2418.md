---
title: 'wrkslots-census-own-descendants: a remove can refuse on a detached helper its own git calls left in the slot, so wrkslots should wait for its own descendants before a census'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- safety
- testing
created_at: 2026-11-27T19:40:00-05:00
updated_at: 2026-11-27T20:15:00-05:00
---

# Description

A host's `git` can be a wrapper that leaves a detached helper behind after each git call. The helper outlives the git call by tens to hundreds of milliseconds, and it runs git again with the checkout as its working directory. When it is orphaned, it is reparented to init or to the nearest subreaper. It is no longer a descendant of wrkslots.

A wrkslots remove makes many git calls in the slot. Its census then refuses on any live process whose working directory is in the slot: "live process N uses slot ... (working directory); stop it and retry". If the census reaches one of these helpers while it lives, the remove refuses on a holder that its own git calls created.

Evidence (2026-11-27, #2065 Step A'):

- In a PID namespace, the Rust/Python remove differentials in `py/wrkslots/tests/test_remove_write_rust.py` refused this way in 2 gate shards and in 7 local runs.
- A sampler of every process whose working directory was in a test slot named the holder each time: the wrapper's helper, running `git remote get-url origin` in the checkout, orphaned to the namespace init.
- Each Rust remove left about 30 to 65 such helpers in its slot.
- The Rust post-fence census runs right after the remove's own git calls. With few processes in the namespace, it is fast enough to reach a helper.
- On the full host table the census takes tens of seconds, and the Python census walks the fenced tree first, so both usually miss the helpers. The race exists there too, and it can explain some "live process uses slot" refusals on busy hosts. That is plausible but not shown.
- The 9 affected differentials now run only on the host and only on opt-in (`ordinary_environment` plus `load_quarantine`, #2065). They come back to the namespaced shards when this issue is fixed.

Neither edition lets a host choose the git binary or its environment for wrkslots' own git calls. The Rust edition starts `git` from PATH at every call site, and so does the Python edition's main git runner.

# Design

This is a removal-path change. It needs a cold review, both editions with parity, and it waits behind #2392 under the hold on removal changes. No process is special-cased by name or command line.

**P1 (recommended): wrkslots adopts and waits for its own descendants.**

- At the start of a remove (and of any operation that runs a census after its own commands), wrkslots sets `PR_SET_CHILD_SUBREAPER` on itself. Anything its own commands leave behind is then reparented to wrkslots instead of init.
- Before each census, wrkslots lists its own children from `/proc/self/task/*/children` and waits for each pid by pid. It never calls `waitpid(-1)`, so it never steals the exit status of a subprocess another part of the program is waiting on.
- The wait is condition-based (a pidfd or a blocking wait per child) with a generous bound.
- A child still alive at the bound is a holder like any other, and the remove refuses as it does today, naming it.
- Safety: no live process is ignored and no outside process is adopted. Only wrkslots' own descendants are allowed to finish before the census reads the table.
- Trade-offs:
  - A descendant that escapes adoption (for example into a new PID namespace) is not covered, and the remove still refuses on it, which is the safe direction.
  - Each census first waits out the helpers' short lifetimes.
  - Python code that calls `os.wait()` or `waitpid(-1)` while a remove runs must be audited.
  - The subreaper flag is not inherited across fork.

**P2 (alternative): one bounded re-check.**

- When the census names a holder, wait for that pid to exit (condition-based, with a bound), then run ONE fresh full census, and refuse if any holder is found.
- This keeps the safety model: deletion still requires a census that finds no holder, and the path fence still blocks new entry by path.
- Trade-offs:
  - It does not remove the cause. A remove leaves dozens of helpers, so the fresh census can name the next one, and one re-check may not be enough.
  - It delays a real holder's refusal by the bound.
  - On a large host the refusal path costs a second full census.

**Optional knob: `WRKSLOTS_GIT=<absolute path>`.** It names the git executable for wrkslots' own git calls in both editions. The default stays `git` from PATH, and a relative path is refused at startup. It lets a host point wrkslots at a git that leaves no helper. P1 makes it unnecessary for this cause, so it is offered separately.

# Acceptance Criteria

- A remove whose own git calls leave short-lived detached descendants in the slot proceeds, with no refusal naming one of them. A test proves this in both editions with a fake `git` on PATH that leaves such a helper.
- A live outside process using the slot still refuses, with its pid named, in both editions.
- A descendant that outlives the bound is refused as a holder, with its pid named.
- The 9 `test_remove_write_rust.py` differentials marked for this cause pass in the namespaced shards with their marks removed, in a gate.
- No new persisted wrkslots field. No wall-clock budget in a gating check (owner rule 346); the bound is a generous cap on an event wait.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-27), from the #2065 Step A' final gate (aa943857) and the local namespace runs that named the holder. Not staffed yet.

Host opt-out trial (2026-11-27, one paired trial, load1 about 220). The wrapper on the affected host has its own switch that turns off its helper.
- Without the switch, one git call made with the checkout as its working directory left one orphaned helper in the checkout, seen 16 ms after the call.
- With the switch set, the same call left none.
- The result comes from one call each, not from documentation.
- The switch is a host mitigation only. It is not repo content, and it is offered to the other team as an optional production setting.
- It is NOT set in this harness's gate environment, because it would hide the race that this issue fixes.

Related:
- #2065: hot-host census tests; the marked differentials and the inventory.
- #2392: removal data-loss fixes. This issue waits behind it.
- #2402: the busy-holder scan's fail-closed follow-ups.

---
title: 'gate-preflight-issue-parse-and-fmt: gate.py preflight refuses admission when an issue file is unparseable or a touched crate is unformatted'
status: closed
priority: 1
issue_type: task
assignee: cl-flaketriage
labels:
- gate
- hygiene
created_at: 2026-11-26T17:20:00-05:00
updated_at: 2026-11-27T13:52:00-05:00
closed_at: 2026-11-27T13:52:00-05:00
---

# Description

`scripts/gate.py` preflight (`PREFLIGHT_STEPS`: `repository.hygiene.client-names` and `python.repository-infrastructure.package-contract`, about 20 s through the validation graph) refuses a top-level run before it queues when those two content checks fail. Two other deterministic content failures still cost a whole gate: `repository.hygiene.issue-parse` (`python3 scripts/check_issue_parse.py`, the pinned-`mb` reader over `.minibeads/issues/`) was red on 35 distinct gate heads in the last 7 days, and `vibe-talk.check.fmt` / `repository.format.rust` (`cargo fmt --check`) on 13, each discovered only after queueing and running a full gate. An unparseable issue file breaks `mb` repo-wide, and an unformatted crate fails identically on every run, so neither failure can be load or luck (owner rule 346).

# Design

Extend the gate preflight, in the preflight code only, so a top-level run is refused before any admission slot is used (same `PREFLIGHT_EXIT` 65 path, ledger `kind=preflight-refused`) when either check fails, with a clear message naming the failing check and its output:

- Issue parse: when the change touches `.minibeads/`, add `repository.hygiene.issue-parse` to the preflight steps; on failure refuse with its named bad files and `mb` error lines.
- Formatting: add the rustfmt step (`cargo fmt --check`, no build) of each Rust workspace the change touches: `rs/` maps to `repository.format.rust`, `vibe-talk/` to `vibe-talk.check.fmt`, and a root `rustfmt.toml` change to both. The touched set is the paths that differ from the gate base (`github/main`), committed, uncommitted and untracked, the same set `validate.py --base` selects from; when it cannot be determined, both fmt steps and the issue-parse step run and the message says so. Never build.

Keep the existing two `PREFLIGHT_STEPS` and every other preflight behaviour (nested gates and `--rerun` skip it, `--skip-preflight` overrides and is recorded, `$GATE_PREFLIGHT_COMMAND` still replaces the checks for tests). The preflight stays contained like a gate (`_contained_command`, per-gate CPU cap, nice 10).

# Acceptance Criteria

- A top-level `gate.py` run on a tree with an issue file the pinned `mb` refuses exits `PREFLIGHT_EXIT` (65) before taking an admission slot, prints the bad file and `mb` error, and records a `preflight-refused` ledger row; no gate slot is used.
- A top-level `gate.py` run on a tree whose touched crate fails `cargo fmt --check` exits 65 before admission with the rustfmt diff named; a formatted touched crate passes; workspaces the change does not touch are not checked (or, when the touched set is indeterminate, the fallback is named in the message).
- Tests cover both refusals and the pass-through (formatted / parseable tree reaches admission), the touched-path to step mapping, and that nested and `--rerun` invocations still skip the preflight.
- No build is run by the new fmt checks (rustfmt only), and no wall-clock budget is added (owner rule 346).

# Notes

Brief `mu-flaketriage`, taken over by `cl-flaketriage` (coordb, 2026-11-26). Team-tick frequent-red counts (7 days, distinct gate heads): `repository.hygiene.issue-parse` 35, `vibe-talk.check.fmt` 13. Sequencing: base on main after `#2207` (touches `scripts/gate.py` and `validate.py`) lands; keep the `gate.py` hunk in the preflight code only. Related: `scripts/check_issue_parse.py` (`#951` mb-id-collision-guard), `validation/repository.dag.yaml` (`hygiene.issue-parse`, `format.rust`), `vibe-talk/tests/dag.d/000-fmt.yaml` (`check.fmt`).

## Resolution

[Claude Opus 5.5, cl-flaketriage] Landed on main as ea46dd3e, replayed from 514c7971 on dfa74c15.
- Gate: a full gate on 514c7971 failed only on python.wrkslots.shard-5. That failure is a known-red census test, test_census_late_fork, in a component this change does not touch. An exclusive rerun of that shard passed: 154 passed, load1 143-150.
- The change: the gate preflight adds repository.hygiene.issue-parse when the change touches `.minibeads/`. It also adds the rustfmt step of each Rust workspace (`rs/` -> repository.format.rust, `vibe-talk/` -> vibe-talk.check.fmt; a root rustfmt.toml adds both) when that workspace has a changed Rust source, manifest or rustfmt config.
- The touched set is the one `validate.py --base` selects from: commits since the merge base, uncommitted edits and untracked files.
- When the touched set is unknown, every conditional step runs and the message says so. A refusal goes through the existing preflight path (exit 65, before admission) and names the command to run to fix it. rustfmt only, never a build.
- Tests in py/tests/test_gate_admission.py:
  - the conditional steps are real graph steps with fix hints;
  - the touched-path to step mapping, including the unknown-change fallback;
  - the selection over committed, uncommitted and untracked work in a scratch repository;
  - a refusal on issue-parse or rustfmt names the fix.
- The existing nested and `--rerun` preflight skips are unchanged. No persisted fields. No wall-clock budget.

---
title: 'wrkslots-census-hot-host: the same-UID process census cannot close on a busy host, so remove --validate-complete refuses in working hours'
status: open
priority: 1
issue_type: bug
assignee: cl-census
labels:
- wrkslots
- census
created_at: 2026-11-27T08:10:00-05:00
updated_at: 2026-11-27T08:10:00-05:00
---

# Description

Trusted consumer evidence, 2026-11-27 (agent-utils pin 132dd98d): `wrkslots remove --validate-complete --census-seconds 600` on an inert validation slot (recorded owner dead, no process with its working directory in the slot) returned rc 3 after 247 s of wall time (0.07 CPU-seconds) with:

"REFUSED: same-UID process census did not close: 8 rounds (not necessarily consecutive) of 10 inventories found generations that forked or exited before their scan completed (324 generations in all, 15 in the last such round); retry when fewer processes are starting"

Seven or eight build and test runs were active, which is normal on that host (about 5,000 processes). The same census gates the validation launcher's teardown path, so teardown refuses there too.

Cause: `_capture_same_uid_process_path_census_following` in `py/wrkslots/cli.py` counts a round as proven only when none of its new generations exited, was replaced or changed before its scan completed, and closes only after two proven rounds in a row. On a host with thousands of process and thread births per second, most new generations are gone within milliseconds, so almost every round has one, and the followed-round bound refuses. #1883 tracks the same refusal; its general-census direction is still open.

Not this issue: #2065 and #1957 (tests that depend on the live process table), #2372 (the mount-holder scan), #2337 (plain `remove --validate-complete` reaching the existing validation teardown closure).

# Design

Safety change: design first, cold review (coordb and the consumer's lead) before any code. Design note: `ai_docs/plans/2026-11-27-wrkslots-census-hot-host-design.md` in the harness repository. It covers what the census proves today, the threat after the fence, the candidate closures, the recommendation with its safety argument, the test plan and the persisted fields (none expected).

# Acceptance Criteria

- An inert slot with a dead owner is removed by `remove --validate-complete` on a busy host (load1 at or above 237 for the whole run, recorded), and the validation teardown path closes the same way.
- A live holder still refuses under the same churn, including a holder's child that inherited the reference after its parent exited.
- Hermetic tests with injected inventories: steady churn of 5,000 processes, a live holder, a child of a holder, negative controls; the existing census suites pass unchanged.
- No new persisted field; if one proves necessary, it is reported to coordb before landing.

# Notes

Filed by cl-census (Claude lane) at coordb's direction; the number was pre-assigned by coordb.

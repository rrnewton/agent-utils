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
updated_at: 2026-11-27T10:00:00-05:00
---

# Description

Trusted consumer evidence, 2026-11-27 (agent-utils pin 132dd98d): `wrkslots remove --validate-complete --census-seconds 600` on an inert validation slot (recorded owner dead, no process with its working directory in the slot) returned rc 3 after 247 s of wall time (0.07 CPU-seconds) with:

"REFUSED: same-UID process census did not close: 8 rounds (not necessarily consecutive) of 10 inventories found generations that forked or exited before their scan completed (324 generations in all, 15 in the last such round); retry when fewer processes are starting"

Seven or eight build and test runs were active, which is normal on that host (about 5,000 processes). The same census gates the validation launcher's teardown path, so teardown refuses there too.

Cause: `_capture_same_uid_process_path_census_following` in `py/wrkslots/cli.py` counts a round as proven only when none of its new generations exited, was replaced or changed before its scan completed, and closes only after two proven rounds in a row. On a host with thousands of process and thread births per second, most new generations are gone within milliseconds, so almost every round has one, and the followed-round bound refuses. #1883 tracks the same refusal; its general-census direction is still open.

Not this issue: #2065 and #1957 (tests that depend on the live process table), #2372 (the mount-holder scan), #2337 (plain `remove --validate-complete` reaching the existing validation teardown closure).

# Design

Safety change; designed first and cold-reviewed four times (coordb, the consumer's lead and its review seat) before any code. Design note: `ai_docs/plans/2026-11-27-wrkslots-census-hot-host-design.md` in the harness repository.

Scope agreed on 2026-11-27 (coordb): **v1 = B + C**.

- B: today's census stays the proof, unchanged (readers, vanished-generation barrier, bounds, refusal texts, exit 3). A non-closing census adds a final line `census-inconclusive: <cause>` and, where JSON is printed, an output-only field `census_inconclusive`. Causes are set at the raising site: vanished-generations, live-generations-unresolved, evidence-kept-changing, census-time-bound, census-resource-limit, evidence-unreadable, census-visibility, census-parse-error, and a fail-closed census-unclassified. A positive keeps its text.
- C: a documented retry recipe (backoff on the retryable causes, stop on the others) for callers such as a consumer's removal queue. The busy-host refusal on shared-image subvolumes is NOT fixed by v1.

Withdrawn as proposals after cold review: a PID-cursor closure, a loop-device claim, and an owner-only write lease on per-slot images. The owner-only lease cannot exclude an ordinary same-UID opener of the raw image once the kernel force-breaks the lease. A sound per-slot-image path needs a new kernel-level authority (a root fanotify open-permission mark, or fs.lease-break-time=0); that is an owner decision recorded in the design note's research section.

# Acceptance Criteria

v1 (this issue's code):

- Every refusal site reached from the same-UID census carries exactly one cause, proven by a test that enumerates the sites; an unmapped site yields census-unclassified and still refuses.
- For each cause, the existing refusal text is unchanged byte for byte and is followed by the cause line (and the JSON field where JSON is printed); positives are unchanged.
- The existing census suites pass unchanged; no persisted field changes.
- The causes and the retry recipe are documented in the wrkslots user guide.
- On a busy host (load1 at or above 237 for the whole run, recorded), a census that does not close names its cause, and a scripted retry later closes or names again.

Still open, NOT met by v1: removing an inert slot by `remove --validate-complete` on a busy host. This stays open pending the owner's decisions: the per-slot-image research path in the design note, and #1883 for shared-image subvolumes and plain worktrees.

# Notes

Filed by cl-census (Claude lane) at coordb's direction; the number was pre-assigned by coordb.

Follow-ups filed from this design: #2379 and #2380.

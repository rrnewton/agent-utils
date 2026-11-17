---
title: 'wrkslots-validate-deleted-subvolume-binds: remove validate rows whose subvolume was deleted under a live bind'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-17T08:03:44-05:00
updated_at: 2026-11-17T08:03:44-05:00
---

# Description

[watermelon-full-preview] Two consumer defects from wrkslots build
6e80ecfb, verbatim facts:

5. A validate row whose btrfs subvolume was deleted while its bind
mount stayed mounted cannot be removed. `remove --validate-complete`
refuses with "validation-batch seal journal.targets[0].identity[0]
must be an integer >= 1": the seal records st_dev 0 and then rejects
it. stat of the checkout dir gives dev=0, ino=256, an empty directory.
Nothing was written and there is no journal. findmnt shows each
checkout (three validate-fresh-* rows) mounted from
/dev/loop1[/<slot>//deleted] btrfs, and each is ALSO mounted at a
second mirror path (a home-directory sync mirror) with the same
source.

6. Some rows outlived their directory: a pressure-* row and four
validate-fresh-* rows whose checkout directories no longer exist.

The consumer holds the real rows and will run the fixed build
themselves; do not touch their rows.

# Design

- Detect a deleted-subvolume source (the mountinfo root ends in
  //deleted, and/or st_dev 0 on the checkout); never write a 0
  identity into a seal or journal.
- Classify such a row as absent storage: remove/recover unmount the
  stale binds wrkslots owns (the slot path, and only mirror paths
  whose source is that same deleted subvolume; never anything else),
  then reconcile the row exactly like recover-absent-validate-rows.
- Fail closed when a live process still has its cwd or open files
  under the bind (reuse the existing census).
- Check whether recover-absent-validate-rows already handles rows
  whose directories are gone (defect 6); fix it if not.
- Apply the same fix in the Rust edition if it has these paths.

# Acceptance Criteria

- Deterministic tests reproduce both defects: fake mountinfo with a
  //deleted source plus a stat of dev 0; and a row with a missing
  directory.
- A deleted-subvolume validate row is reconciled as absent storage
  after its stale owned binds (slot path plus same-source mirrors
  only) are unmounted, with no 0 identity ever sealed or journaled.
- A live process using the bind refuses the removal (fail closed).
- Rows whose directories are already gone are handled, with a test
  proving it.

# Notes

[watermelon-full-preview] Filed by mu-deadbind. Overlap: mu-lockedreg
(#1766), mu-censusbound (#1747) and mu-censusexit (#1769) are changing
remove/recover/census code in py/wrkslots/cli.py concurrently; keep
this change small and local and rebase right before the gate.

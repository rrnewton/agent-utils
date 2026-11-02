---
title: 'wrkslots-recover-census-refusals: wrkslots recover cannot finish an interrupted recover-absent-validate-rows on a loaded host'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-02T21:19:52.029848242+00:00
updated_at: 2026-11-02T21:19:52.029848242+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host (consumer harness, wrkslots from an agent-utils pin older than 3f2fed7d): the machine journal holds an interrupted recover-absent-validate-rows for one validate slot (no live owner), and it blocks wrkslots mutations there. 'wrkslots recover --coordinator-pid <pid>' refuses in its census three ways: (1) 'mount namespace is indeterminate for live PID N' for transient PIDs that had exited, at load 240-400 (the exit-teardown shape fixed by 3f2fed7d?); (2) 'REFUSED: privileged read-only census stderr exceeded 4101 bytes' (a stderr bound too small for a busy host's census); (3) warnings about 8 lingering PID-namespace inits being torn down. recover also needs the coordinator PID in the invoking ancestry, so a helper cannot run it. Asks: (a) confirm whether running recover with a wrkslots from main >= 3f2fed7d against that project root is safe and sufficient for (1); raise or make configurable the census stderr bound for (2); (b) say whether the torn-down PID-namespace inits are safe to leave (current code classifies them as holding nothing) and what reaps them; (c) a supported way to complete or abandon one interrupted recovery (no hand-editing of the journal), and a documented way for an authorised helper to run recover.

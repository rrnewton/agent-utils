---
title: "rust-census-privileged-fallback: Rust remove/audit escalate protected and foreign processes to a root census like Python"
status: open
priority: 1
issue_type: feature
created_at: 2026-11-10T05:45:00.000000000+00:00
updated_at: 2026-11-10T05:45:00.000000000+00:00
---

# Description

The Rust edition's remove and audit share one unprivileged process snapshot (`capture_process_snapshot`). When a
process's /proc entries (cwd, root, exe, fd links, maps) are unreadable while the process is alive and not
exiting, that is a protected process: non-dumpable or setuid-transitioned, e.g. `systemd --user`, `(sd-pam)`,
`gnome-keyring-daemon`, a privileged helper. Or it is a foreign-uid process. In both cases Rust treats the
unreadable entries as no evidence (links since #1102/#618, maps since #1174). Python's audit and batch/recovery
censuses do not: `_same_uid_direct_path_processes` sends any process whose probe gives PermissionError to the
privileged fallback (`_absent_validate_find_matches` / `_absent_validate_maps_matches` via `_run_root_owned_command`,
root find/grep through `sudo -n`), and the census refuses if that fallback is unavailable. So in this case Rust audit
is weaker than Python audit in the refusal direction. One measured host had 4 permanently unreadable own-uid
processes among about 1,064.

# Design

- Add the same privileged fallback to the Rust census: for each process whose entries are unreadable and that is
  not exiting (the exiting case keeps the #1173 bounded recheck), run the root-owned matchers Python uses (same
  commands, same `sudo -n` non-interactive rule, same output parsing). Count a match as use; if the privileged
  command is unavailable or fails, refuse as Python does.
- Mirror Python's split exactly: the ordinary remove path (lsof first, unreadable is no evidence) versus the
  audit and batch/recovery censuses (escalate). Rust must never be weaker than Python in the refusal direction.
- Never run real sudo in tests. Stub the privileged runner as the Python tests do.
- Remove the gap note #1174 added to the USER_GUIDE once this lands.

# Acceptance Criteria

- The protected-process differential case that #1174 added (asserting today's documented Rust no-evidence
  behaviour) flips to match Python's escalation: a stubbed root matcher that reports use makes both editions refuse,
  and an unavailable privileged runner makes both refuse.
- A foreign-uid process case behaves identically in both editions.
- The ordinary remove path keeps Python's no-evidence rule for protected processes, proven by a differential.
- No real sudo anywhere in tests.

# Notes

Filed for the lead coordinator (2026-11-09) with the #1174 rust-remove-census-root-maps ruling: protected
non-exiting processes stay no evidence in Rust for now, documented, and this issue closes the gap. Reference
implementation: the Python audit census path named above.

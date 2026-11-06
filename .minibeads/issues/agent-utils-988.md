---
title: 'agentctl-container-pidns: port the foreign PID-namespace pane translation to the Rust edition'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T23:53:37.628349076+00:00
updated_at: 2026-11-06T23:53:37.628349076+00:00
---

# Description

[opus 5.5] 33dfe6c5 taught the Python agentctl edition (py/agentctl/pidns.py, HerdrClient.process_info/pane_info) to detect a Herdr server outside this PID namespace (SO_PEERCRED peer pid 0) and either translate a podman/docker exec pane onto local processes (exec root, terminal foreground group, native-session file holder) or refuse process proofs with 'runs outside this PID namespace'. The Rust edition, which is now primary, has neither: inside a container under a host Herdr server it compares the host pane cwd (adopt/status refuse with cwd mismatch) and, worse, would read host PIDs from local /proc, where the same numbers name unrelated container processes. Until ported, run AGENT_UTILS_ENGINE=python for such panes. Acceptance: the Rust edition passes the cases in py/tests/test_agentctl_pidns.py (translation, every refusal, unchanged local behaviour) and a differential case covers it.

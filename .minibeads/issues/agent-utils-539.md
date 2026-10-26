---
title: 'rust-primary-mirror-state: machine-readable mirror-state model and format inventory'
status: open
priority: 2
issue_type: task
created_at: 2026-10-26T16:11:15.615056145+00:00
updated_at: 2026-10-26T16:11:15.615056145+00:00
---

[muse watermelon] rust-primary campaign milestone 0 (plan of record step 0).

Add `validation/editions.json`: per tool and per subcommand, the primary edition
(rust|python), the other edition's state (parity | paused-at-<commit> | archived |
absent), and the parity check that enforces it. The engine resolver and
`make validate` read it; a paused/archived/absent mirror refuses clearly instead
of silently falling back.

Include an inventory of every on-disk/wire format (queue items, registry records,
coordinator.json, landing-queue.json, wrkslots state/handoffs, wrkviz archive
records, chat bridge state, teams/declarations, validation JSON): where its
definition lives today, duplicates flagged (including Rust-vs-Rust), and a
collapse plan to one Rust type plus a generated versioned schema per format
(the vibe-talk `web/contract.js` pattern). Document the model in README.

Acceptance: `scripts/check_editions.py` (with `--self-test`) proves editions.json
matches the tracked `rs/bin`/`py/bin` entrypoints and the Rust agentctl
python-only subcommand lists; a hygiene DAG node runs it on every change; the
engine resolver consults it when present and keeps its presence-based fallback
for partial checkouts (its sandbox tests).

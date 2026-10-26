---
title: 'rust-primary-rust-only-check: a no-Python smoke check that meters the campaign'
status: open
priority: 2
issue_type: task
created_at: 2026-10-26T18:38:09.654579471+00:00
updated_at: 2026-10-26T18:38:09.654579471+00:00
---

[muse watermelon] rust-primary campaign milestone 1 (plan of record step 1).

A Rust-only mode check: run the dispatched tools' smoke invocations with no
Python interpreter on PATH and report, per tool, whether it runs, still needs
Python, or fails. This is the campaign's progress meter: it must fail (exit 1)
when a Rust-primary tool cannot run without Python, and otherwise print the
meter and exit 0. Deliverables: scripts/check_rust_only.py (stdlib, with
--self-test and a durable pytest), a validation DAG job that runs it after
the Rust launchers are built, and a container recipe (validation/rust-only/)
whose image has a Rust toolchain and no Python. The editions model
(validation/editions.tsv) is the source of which tools are expected to pass.

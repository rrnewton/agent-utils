---
title: 'rust-primary-wrkviz: port wrkviz to Rust behind the same CLI'
status: open
priority: 2
issue_type: task
created_at: 2026-10-30T06:44:30.605209022+00:00
updated_at: 2026-10-30T06:44:42.826876431+00:00
---

# Description

[muse watermelon] rust-primary campaign milestone 5: port wrkviz to Rust incrementally behind the same CLI. Python wrkviz is ~60k lines with thin tests, so the first slice builds a test corpus (fixture session/archive inputs plus golden outputs from the Python edition) before any Rust code. The archive record formats (schema 2 and schema 3) get exactly one Rust type as source of truth with a generated schema the Python mirror validates against. The static site assets (JS/CSS/HTML) carry over unchanged. Port subcommand by subcommand with differential tests; switch primary in validation/editions.json only at parity. Predecessor: #562 rust-primary-wrkslots (slices 2-58 landed; wrkslots audit has no funded Python-owned corners left).

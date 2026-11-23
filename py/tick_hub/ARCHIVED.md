# tick_hub (Python edition) — ARCHIVED

This Python edition of tick-hub is frozen in place and is no longer maintained
or kept at parity with the Rust edition. It was archived on 2026-11-22 at
commit 2f4f2e132d3879e98fe76a217ff99c1a7d583258, when it was equivalent to the Rust edition at commit
2f4f2e132d3879e98fe76a217ff99c1a7d583258 (tick-hub 0.2.0). Annotated tag:
`archive/py-tick-hub-2026-11-22`. Tracking issue:
#2139 python-editions-archive-and-parity-retirement.

The live tool is the Rust edition (`rs/tick-hub/`; `cargo install tick-hub`).
Config and ops-state files written for this edition remain readable by the
Rust edition; no persisted format changed at archival.

Known frozen divergence: this edition's `tick --dry-run` flag has no Rust
counterpart (the Rust default already persists nothing without `--flush`).

To revive this edition: check out the tag above, restore the
`cross.tick-hub.differential` job in `validation/cross.dag.yaml` and the
tick-hub entries listed in the archival commit, and bring the edition back
to differential parity before changing `validation/editions.json`.

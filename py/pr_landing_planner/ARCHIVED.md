# pr_landing_planner (Python edition) — ARCHIVED

This Python edition of pr-landing-planner is frozen in place and is no longer
maintained or kept at parity with the Rust edition. It was archived on
2026-11-23 at commit 73892095cdb70a3536ba3e71d43a0fa9e6f8413d, when it was equivalent to the Rust edition
at commit 73892095cdb70a3536ba3e71d43a0fa9e6f8413d (pr-landing-planner 0.1.0). Annotated tag:
`archive/py-pr-landing-planner-2026-11-23`. Tracking issue:
#2139 python-editions-archive-and-parity-retirement.

The live tool is the Rust edition (`rs/pr-landing-planner/`; `cargo install
pr-landing-planner`). Fixture and landing-context files written for this edition
remain readable by the Rust edition; no persisted format changed at archival.
The engine resolver refuses an explicit Python-edition request for this tool;
`py/bin/pr-landing-planner` invoked directly still executes this frozen edition.

The Rust `merge-queue` crate depends on the Rust planner crate only; it never
used this Python edition and is unaffected by the archival.

To revive this edition: check out the tag above, restore the
`cross.planner.differential` job in `validation/cross.dag.yaml` and the planner
entries listed in the archival commit, and bring the edition back to differential
parity before changing `validation/editions.json`.

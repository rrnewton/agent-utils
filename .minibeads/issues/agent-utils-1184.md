---
title: "wrkslots-machine-auto-rust: Rust parity for optional machine and doctor machine_source"
status: closed
priority: 1
issue_type: task
created_at: 2026-11-10T02:42:52.000000000+00:00
updated_at: 2026-11-10T02:42:52.000000000+00:00
---

# Description

Python #1163 (d0f81236) made `machine:` optional in `.wrkslots.yml`: an
absent key or the literal `auto` resolves to the short host name at load,
so one tracked configuration can serve every host. The Rust config reader
still requires `machine`, so a shared configuration that drops it is refused
by the Rust edition even though Python accepts it. That also blocks a
consumer from adopting the shared form while any of its hosts may route
through Rust.

# Design

Mirror the Python loader exactly, in the core config path so every Rust
command inherits it:

- `machine` becomes optional in the Rust reader and validator required-key
  sets; the normalized config always carries the resolved machine name.
- An absent key or the literal `auto` resolves to the short host name (the
  host name up to the first dot), name-validated the same way.
- Precedence is unchanged: `--machine`, then `WRKSLOTS_MACHINE`, then the
  configured value (where absent/`auto` means the host name). Overrides stay
  literal. The config records the same `machine_source` values as Python.
- `doctor` shows `machine: <name> (host name; ...)` in human and JSON output
  only for host-name sources; explicit configuration output stays
  byte-identical. The `--machine` help wording matches Python, and the
  already-mirrored `config_docs.rs` comment stays in sync.

# Acceptance Criteria

- Rust accepts a config without `machine` and one with `machine: auto`, and
  resolves the short host name in both; an explicit value still wins.
- `--machine` and `WRKSLOTS_MACHINE` still override all three configured
  forms, and `machine_source` matches Python in the normalized config.
- Doctor prints the host-name note (human and JSON) only for host-name
  sources, with strings copied byte for byte from Python.
- Differential tests cover absent, `auto`, explicit, environment override,
  and flag override against the Python oracle.

# Notes

Split from #1163 at the lead coordinator's request on 2026-11-09: the Rust
half is P1 because it blocks re-enabling the reader soak once shared
configurations start dropping `machine:`.

## Resolution

[muse watermelon] Done. `rs/wrkslotsd/src/config_validate.rs` no longer
requires `machine`: an absent key, a null value, or the literal `auto`
resolves to the short host name read from `/proc/sys/kernel/hostname`,
name-validated like any other machine. Precedence matches Python: a
non-empty `--machine`, then a non-empty `WRKSLOTS_MACHINE`, then the
configured value. The normalized config carries the same `machine_source`
values as Python (`--machine`, `WRKSLOTS_MACHINE`,
`host name (machine absent)`, `host name (machine: auto)`,
`configuration.machine`). `doctor_state.rs` prints
`machine: <name> (<source>)` in human output and a `machine` field in JSON
output only when the source is a host-name source, with the Python strings
byte for byte; explicit-configuration output is unchanged. The `--machine`
help on the config and doctor commands states the absent/`auto` meaning.

Proof: `test_validate_config_machine_resolution_matches_python` and
`test_validate_config_machine_overrides_match_python` in
`py/wrkslots/tests/test_config_validate_rust.py` compare the full normalized
config against the Python oracle for absent, `auto`, explicit, environment
override, and flag override; `test_doctor_machine_auto_note_matches_python`
in `py/wrkslots/tests/test_doctor_state_rust.py` compares doctor human and
JSON output byte for byte. The full config and doctor differentials pass
(51 tests).

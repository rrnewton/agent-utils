---
title: 'wrkslots-config-forward-compat: an older wrkslots refuses every new optional config key, so each one is a fleet-wide flag day'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-09T03:05:00.000000000+00:00
updated_at: 2026-11-09T03:05:00.000000000+00:00
---

# Description

Unknown top-level keys in `.wrkslots.yml` hard-refuse (`_load_config` -> `_exact_keys` -> "configuration has invalid fields: unknown <keys>"). So every new optional key (`identity` from #1096, `reclaimable_ignored_paths` from #1088) can be used only once every reader of that config has been upgraded.

On one consumer host, 23 owner-live slots (20 agent, 3 validate) still run older agent-utils pins, and its launcher points every slot at the one shared config in the primary checkout. Adding either key would make every old-pin slot refuse, even read-only `status`. The consumer cannot roll out #1096 or #1088.

# Design

The lead's design, which keeps typos failing closed:

1. A top-level `ignorable_keys: [identity, ...]` list. A reader that doesn't know a key ignores it, with a one-line warning naming the key and the reader's version, ONLY if the config lists that key in `ignorable_keys`. An unlisted unknown key still refuses, so a typo like `max_active_slot` is never silently ignored.
2. A `min_wrkslots_version` (or the existing schema integer) that writers bump only when a key is load-bearing for correctness. An older reader then refuses naming the required version, instead of the generic invalid-fields error.
3. Both new fields are understood from this release on: one last flag day, after which additive keys roll out incrementally.

Document the rule in the config reference ("additive optional key: list it in ignorable_keys; correctness-bearing: bump min_wrkslots_version"). Mirror it in Rust (config_validate.rs).

# Acceptance Criteria

- Tests with an old-key-set reader fixture, in both directions: a listed unknown key is ignored with the warning; an unlisted unknown key refuses; a typo refuses; `min_wrkslots_version` above the reader's version refuses naming the version.
- Python and Rust agree (differential test).
- The config reference documents the rule and both fields.

# Notes

Requested by the lead (devbig014), 2026-11-08, from a consumer's rollout blocker. Sequenced after #1108 and ahead of #1106/#1107. Related: #1088, #1096.

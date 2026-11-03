---
title: 'agent-record-schema-forward-compat: versioned agent-record schemas tolerate keys a newer version added'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-03T10:14:01.600972177+00:00
updated_at: 2026-11-03T10:14:01.600972177+00:00
---

# Description

[opus 5.5] Follow-up to #809 chat-bridge-json-forward-compat. That change made every serde-decoded persisted agentctl file read around keys a newer version added. Several agent-registry documents are instead checked by hand for their exact key set, in both editions. A key a newer agentctl adds there still makes an older one refuse the whole record or journal ("invalid agent record ... fields", "invalid launch spec (fields)", "invalid rename journal"). It is the same hazard as #809, under a different contract: each document carries a schema version, and today a new field means a new version.

Hand checks (exact key sets), per schema:

- Nested agent-record rows `agentctl-session/v2` and `agentctl-session/v3` (top-level fields).
  - v2 adds session_agent, session_value, goal, goal_delivery, goal_session_id, goal_command, goal_messages, goal_message_id and custom_process_identity.
  - v3 adds native_session, goal and custom_process_identity.
  - Rust: rs/agentctl/src/subagents/nested.rs decode_nested (`document.keys() ... != expected`). Python: py/agentctl/subagents.py (`set(document) != expected`).
- Their `launch` object, `agentctl-launch/v1` and `agentctl-launch/v2` (v2 adds permission_mode). Rust: nested.rs (`launch.keys() ... != launch_fields`). Python: subagents.py (`set(launch) != launch_fields`).
- Inside those rows:
  - the launch `executable` ({path, device, inode});
  - the v3 `native_session`;
  - the `goal` object (`agentctl-goal/v1`: schema, objective, message_id, native_command);
  - the nested custom process identity.
  - Rust: nested.rs (key-set comparisons for each). Python: subagents.py (`set(executable) != ...`, `set(native) == ...`, `set(goal) != ...`).
- The flat record's `launch_spec` (`agentctl-launch-spec/v1`; its native_session holds agent, value and source). Rust: rs/agentctl/src/subagents/launch_spec.rs validate (`spec.len() != KEYS.len()`). Python: py/agentctl/launch_spec.py (`set(value) != _KEYS`, `set(native) != {"agent", "value", "source"}`).
- Rename journals `agentctl-rename/v1` (`.renames/<token>.json`; each edition finishes the other's). Rust: rs/agentctl/src/subagents.rs RenameJournal validation. Python: py/agentctl/subagents.py _validate_rename_journal (`set(value) != keys`).

# Design

- Decide per schema between two rules:
  - a key a newer agentctl adds is read around, and kept when the document is rewritten, as #809 does for agent records;
  - a new field keeps meaning a new schema version. Then an older build should still read the record as far as it can (or report it as from a newer build), not call it invalid.
- Change both editions together; the cross differential compares them (cross/agentctl_nested_records.py).
- Record the choice per schema in this issue.

# Acceptance Criteria

- For each schema above, a test in both editions writes the document with an extra key, and the current build reads it (or reports it as newer, by design). A rewrite keeps the key where it is kept.
- The cross differential covers at least one of them.

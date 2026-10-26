---
title: 'rust-primary-agentctl-chat-readers: Rust chat history and context readers'
status: open
priority: 2
issue_type: task
created_at: 2026-10-26T19:56:33.565034111+00:00
updated_at: 2026-10-26T19:56:33.565034111+00:00
---

[muse watermelon] rust-primary campaign milestone 2, first slice (plan of
record step 2: flesh out the python-only corners of Rust agentctl, Google
Chat path first).

Implement `agentctl chat history` and `agentctl chat context` in the Rust
edition against the same durable bridge state the Python edition reads,
with differential tests against the Python implementation on fixture state.
These two are pure readers (no network, no harness), so they can move first;
`chat launch` (the polling launcher) is the following slice and stays
python-primary until then. Remove history/context from the Rust edition's
PYTHON_ONLY_CHAT_COMMANDS only when the Rust behavior matches, and update
validation/editions.json in the same commit so the mirror model stays true.

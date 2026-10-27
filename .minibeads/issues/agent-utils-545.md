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

[muse watermelon] Slice landed (f66fd17a + b47763b2): Rust `chat history` only. `chat launch` and `chat context` remain python-primary (context needs live provider transport; launch is the polling supervisor) and are the next slices of milestone 2, so this issue stays open for them.

[muse watermelon] Context slice landed: Rust `chat context` over socket/command adapter transports (built-in provider transport refuses clearly). Remaining in milestone 2: `chat launch`, then mcp/migrate/reset/repair and headless workers. Issue stays open.

[muse watermelon] MCP slice landed: `agentctl mcp` served by the Rust edition (thin JSON-RPC shell over this executable's CLI). Remaining python-only: chat launch, migrate, reset, repair (headless-worker operations). Issue stays open.

[muse watermelon] Reset slice landed (native Rust reset; migrate/repair fold into the herdr-subagents port per coordinator decision). Python-only agentctl surface now: chat launch (held until the chat-delivery-latency lane lands), migrate, repair. Issue stays open.

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

[muse watermelon] Chat launch landed (9086ca74): Rust `agentctl chat
launch` drives the current Rust bridge generation (generation B,
coordinator decision 2026-10-28: the owner retired Google Chat on
2026-10-26 and every live bridge already runs Rust `chat run`, so
the Python polling loop was not ported). It pins the invoking Herdr
shell pane, refuses doomed config/plugin/registry authority before
spawning, spawns the harness through an inert pidfd launch gate,
registers it under the config's agent_name via adopt, creates or
reopens Rust bridge state (a persisted `--agent-label` override
included), spawns `chat run` of the same executable, and supervises
both for the coordinator's lifetime with bridge.log draining,
coordinator exit-code propagation, bridge-first-exit as an error,
and SIGTERM returning 0. A Python-format chat config or Python
polling bridge state is refused with a pointer to `chat init`;
never converted, never read. Tested only with fake providers
(fake herdr, fake harness, chat-subscription-fake-plugin): 8
integration cases in py/tests/test_agentctl_chat_launch_rust.py.
Adversarial review found two Highs (agent-label no-op into state;
relaunch spawning before the registered-name refusal) plus
log-follow, join-bound, CLOEXEC, pre-activation SIGTERM, and
startup-validation findings; all fixed with tests. Known
divergences: state is reopened rather than re-initialized, the
registry record persists after the coordinator exits (lifecycle
tools own it), and `chat run`'s own tuning beyond
--ready-timeout/--reconcile-interval is not forwarded. Gates
49-51: gate 49 caught rustdoc foreign/development-history terms
in the new module (reworded; packages pass all 9 crates); final
commit's own gate passed every agentctl step (Rust tests 1624,
Python shards, 785 + 104 differentials); remaining failures were
load timeouts passed alone (wrkslots shards 535 + 534, infra 113,
vibe-talk keyboard) and host-environment failures proven
identical on untouched origin/main exports (two lifecycle
process-scan tests, the standalone wrkslots lifecycle, a
real-host liveness test that then passed 3/3). Python-only
agentctl surface now: migrate, repair (folded into the
herdr-subagents port). Issue stays open for those.

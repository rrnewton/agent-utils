# agent-utils glossary (Discuss warm-up)

- **mu-\***: the lane naming scheme. Each coding-agent lane is `mu-<task>` (mu-catchup, mu-discussq), working in its own git worktree slot and landing on the shared main branch.
- **coordb / coord7**: coordinator agents. A coordinator delegates work to lanes, supervises them, lands their changes, and reports to the owner. Agents reach each other with `agentctl send NAME`.
- **Herdr**: the terminal multiplexer that hosts agent panes on this machine; lanes run inside Herdr panes.
- **agentctl**: the suite CLI that starts, inspects, messages, and retires named persistent coding-agent sessions.
- **agent-utils**: this repository: a reusable collection of agent libraries and CLIs (agentctl, wrkslots, dagrun, vibe-talk, wrkviz).
- **vibe-talk**: the chat client/server in this repo; its Discuss panel is the catch-up side agent.
- **MCP**: Model Context Protocol; vibe-talk exposes read tools over it.
- **Spark / muse-spark-1.4-internal**: the Muse Spark 1.4 model served by the internal AI gateway.
- **small-elk-internal / Ember**: newer small internal preview models on the same gateway; small-elk-internal is the live Discuss model.
- **P0**: highest priority.
- **wrkslots**: the tool managing isolated git worktree slots, one per agent.
- **dagrun**: the validation DAG runner; `make validate` selects the checks a change can affect.

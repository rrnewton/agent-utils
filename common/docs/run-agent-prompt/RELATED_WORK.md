# run-agent-prompt: related work

| System | What it does | Difference |
| --- | --- | --- |
| Simon Willison's `llm` | Sends prompts to many hosted and local models through one CLI and plugin system | It is a general model interface with its own provider, template, logging, and tool features; `run-agent-prompt` invokes installed coding-agent harness CLIs and adds a fixed local fallback chain |
| `aichat` | Provides a multi-provider terminal assistant with roles, sessions, retrieval, and agent features | It is an interactive assistant surface; `run-agent-prompt` is a non-interactive, single-answer wrapper for degraded startup paths |
| Charm `mods` | Pipes shell input through a language model and formats the answer for pipelines | It targets shell text transformation through model APIs; `run-agent-prompt` targets harness failure, tool disablement, process-group timeouts, and a distinct all-failed exit |
| Aider, Claude Code, Codex, and Muse | Act as coding agents with model, file, shell, and session capabilities | `run-agent-prompt` does not implement an agent; it calls these harness families in restricted one-shot modes and keeps all effects outside the answer |

No system surveyed combines a fixed Muse, Claude Code, Codex
fallback order; prompt transport outside argv; judgement-only
attempt flags; fresh per-attempt workspaces; an allowlisted child environment;
per-attempt and whole-chain deadlines; stdout reserved for the final
answer; and exit 69 when no harness answers.

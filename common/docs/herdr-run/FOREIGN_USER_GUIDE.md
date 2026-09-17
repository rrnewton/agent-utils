# Persistent foreign subagents

`herdr-subagents` lets a coordinator delegate work to named Codex or Antigravity
workers, send follow-up prompts into the same conversations, and read durable
turn results. A terminal window keeps each worker's activity visible to a human.

| Execution mode | Harness | Terminal host |
| --- | --- | --- |
| Headless turns | Codex or Antigravity | tmux or Herdr |
| Interactive TUI | Codex | Herdr only |

In headless mode, a persistent inbox runner starts or resumes the harness for
each turn and records its output and final answer. The conversation persists
between turns; the harness process need not. The terminal displays the runner's
activity rather than a native harness input composer. Headless mode still
requires one of the terminal hosts above.

Interactive mode keeps the native Codex TUI running for direct human inspection
and input. Use `herdr-agent start` for managed Codex and Claude TUIs. Its native
goal operations require a supporting Codex version; other harnesses receive an
objective as text. These commands maintain separate worker registries.

Install the Python distribution of `herdr-run`, which provides `herdr-subagents`
and requires Python 3.10 or newer.

New Codex workers preserve the harness's approval and sandbox settings by default.
Set `SUBAGENTS_CODEX_BYPASS_PERMISSIONS=1` when you explicitly want
`--dangerously-bypass-approvals-and-sandbox` for a headless or TUI worker; `0`
preserves the native settings. Other values are rejected. The registry records
this choice at launch and retains it across later turns and migrations. Existing
registry rows from the earlier runtime, which always enabled bypass, retain that
behavior when they lack the new field.

Set a separate state directory for each workspace. Models remain the harness
default unless you supply `--model`; executable overrides are `CODEX_BIN`,
`AGY_BIN`, and `HERDR_BIN`.

```sh
export HERDR_SUBAGENTS_HOME=/absolute/workspace-state/foreign-workers
herdr-subagents up reviewer --cwd /absolute/worktree --backend tmux --brief 'Review the change.'
herdr-subagents status reviewer
herdr-subagents read reviewer --last
herdr-subagents send reviewer 'Check the error handling too.'
herdr-subagents down reviewer
```

Without `HERDR_SUBAGENTS_HOME`, state lives under
`$XDG_STATE_HOME/herdr-agent/foreign`, defaulting to
`~/.local/state/herdr-agent/foreign`. The registry, archived workers, events,
inboxes, transcripts, and backend preferences stay in this directory.

Available commands are `up`, `send`, `read`, `status`, `down`, `inbox`,
`reset`, `migrate`, `backend`, `turn`, `keeper`, and `mcp`. Each worker command
accepts `--help`. `python -m herdr_run.foreign` exposes the same interface.

Headless turns retain the harness session ID between prompts, serialize their
inbox, capture a durable transcript, and mark each completed turn. `reset`
starts a fresh conversation while preserving the worker identity. `migrate`
can move an idle worker between presentation backends or resume a Codex
headless session as a visible Herdr TUI. TUI reads support bounded scrollback;
`--last` and turn-boundary reads fail explicitly because scrollback has no
durable answer boundaries. Use `read NAME --tail 500` for a TUI.

Backend selection is the explicit flag, `SUBAGENTS_BACKEND`, saved
`backend.json`, consumer policy, then detection. Mode selection is the
explicit flag, `SUBAGENTS_MODE`, the tracked project defaults, then headless.
`HERDR_SUBAGENTS_PROJECT_DEFAULTS` points to a JSON document; it defaults to
`project_defaults.json` in the state directory. For example:

```json
{
  "harness_modes": {
    "codex": "tui"
  }
}
```

`SUBAGENT_EFFORT` selects headless Codex reasoning effort. Senders persist the
value with each queued turn so updates apply to an already-running worker.
Omitting it leaves the harness default in control. `SUBAGENTS_TMUX_SESSION`
and `SUBAGENTS_HERDR_WORKSPACE` select presentation groups; both default to
`subagents`.

Consumers can retain host quota and presentation rules in a Python file
selected explicitly by `HERDR_SUBAGENTS_POLICY`. It defines
`backend() -> str | None` and
`check_launch(harness, model, purpose) -> None`. The latter raises
`AgentOperationError` to refuse a launch. A configured policy that cannot be
loaded fails loudly. No project policy is discovered implicitly.

For an MCP client, configure command `herdr-subagents` with arguments
`["mcp"]` and the same state-directory environment. The server exposes
`subagent_up`, `subagent_send`, `subagent_read`, `subagent_status`,
`subagent_list`, `subagent_down`, `subagent_reset`,
`subagent_recreate_window`, and `subagent_migrate`. It uses newline-delimited
JSON-RPC on standard input/output, with diagnostics on standard error.
The optional local event stream uses `SUBAGENTS_MCP_WS_PORT`.

`down` archives worker state by default. Inspect delivery failures before
retrying: a prompt may have reached the harness even when confirmation was
not observed. Workspace allocation, Git policy, and concurrency limits belong
to the coordinator that calls this runtime.

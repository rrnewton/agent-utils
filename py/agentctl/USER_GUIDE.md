# agentctl user guide

`agentctl` lets a person or coordinator agent manage persistent coding sessions:
start them, submit follow-up work, inspect results, and take over their terminals.
The sessions keep their own native tools, instructions, and conversation. This
is useful when one coordinator needs several long-lived workers that a human
can still examine and steer directly.

## Dependencies and available adapters

The core interactive adapter calls Herdr's public CLI directly. Install Herdr
and the selected Codex or Claude harness separately, and authenticate the
harness before launching workers. The integration uses Herdr's pane, tab,
workspace, agent prompt, agent wait, and session inspection APIs.

The manager itself is a short-lived command. Herdr owns an interactive agent's
terminal process. There is no manager daemon or hosted coordination service to
keep alive. The manager is independent of shell-command execution utilities.

Run `agentctl capabilities` to inspect the modes, backends, harnesses, and
services present in this installation. **The Python distribution includes the
interactive core, headless workers, Chat, and MCP. The Rust distribution provides the
interactive core plus a provider-neutral inbound-subscription protocol and safe
plugin discovery.** It does not ship a provider executable or a Chat bridge
command. The worker, Chat, and MCP extensions remain bundled capabilities of
the Python distribution rather than plugin claims.

| Mode or service | What it controls | Additional dependency |
| --- | --- | --- |
| Interactive | A native Codex or Claude TUI that remains directly accessible | Herdr |
| Headless worker extension | Resumable Codex or AGY structured turns; a terminal shows the transcript | Herdr or tmux and the chosen harness |
| Chat extension | One Google Chat space connected to an interactive coordinator | Google Chat access and Herdr |
| MCP extension | The same session operations over local MCP stdio | An MCP client |

### Rust subscription plugin discovery

The Rust command inspects `~/.agentctl/plugins/` only when `agentctl
capabilities` runs. Ordinary session-control commands do not scan the plugin
directory. This installation provides the protocol, safe discovery, and launch
command builder; it does not provide a durable subscription host command or a
provider executable. Set `AGENTCTL_HOME` to an absolute directory to use an
isolated user-level home.
This home is separate from the project-local session registry selected by
`--registry`; installing a backend cannot change or acquire a project session.
Manifests are discovered automatically in deterministic filename order; there
is no registration command and no project-local plugin registry to update.

Discovery is manifest-only and never executes every file in a directory. One
plugin occupies `plugins/NAME/`, with a private `manifest.json` and the contained
direct-child executable named by that manifest:

See the [normative version 1 process
protocol](https://docs.rs/chat-subscription-plugin/0.1.0/chat_subscription_plugin/) for exact frame
shapes, ordering, durable Commit semantics, and canonical bounds.

```json
{
  "schema": "agentctl-plugin-manifest/v1alpha1",
  "name": "example-chat",
  "capability": "chat-subscription.example",
  "executable": "backend",
  "protocol": {"name": "agentctl-chat-subscription", "min": 1, "max": 1},
  "implementation": "implemented"
}
```

The home, plugin directory, manifest, and executable must be owned by the
current account and must not be group/world writable. Symlinks, hard-linked
files, path traversal, oversized or changing manifests, duplicate capability
identities, and incompatible protocol ranges are refused. `agentctl
capabilities` lists safe compatible entries under `discovered` and rejected
entries under `refused`; it distinguishes these from the built-in subscription
core.

Installers should assemble and validate a complete private staging directory on
the same filesystem, with real single-link `manifest.json` and executable
files, then atomically rename it into `plugins/NAME/`. Upgrades may use an
atomic directory exchange before retiring the old directory. A versioned store
may live outside `plugins/`, but activation must copy real files rather than
symlink or hard-link them into the discovery tree. This keeps discovery from
observing a partial install. Launch reopens every path through pinned directory
descriptors, requires the exact executable identity discovered earlier, and
executes through that pinned descriptor; exchanging the activation directory
before or after command construction cannot redirect execution. Provider
plugins are native executables rather than interpreter scripts, because the
pinned executable descriptor is close-on-exec.

Capability identities name both provider and implementation so alternatives
can coexist, for example `chat-subscription.google-chat.workspace-events` and
`chat-subscription.google-chat.polling`. Reusing the exact capability identity
is a collision: every colliding entry is refused rather than selecting a
filename-order winner.

When a subscription host launches an accepted plugin, it clears the inherited
environment first. Only a small non-secret process baseline (`HOME`, `USER`,
`LOGNAME`, `PATH`, locale variables, `TMPDIR`, and `XDG_RUNTIME_DIR`) is copied.
The manifest cannot request environment names, arguments, or credentials and
cannot self-authorize access to unrelated API keys. Provider credentials belong
to an explicit host-controlled mechanism or to the provider's ambient user
credential service.

A future durable host must retain the returned process supervisor for the
entire subscription. Normal shutdown sends the protocol Close frame through the
public subscription cancellation method, drops both pipes, and waits for a
chosen grace period. The supervisor then kills and reaps within a fixed
two-second backstop; cooperative Close alone cannot interrupt provider code
blocked while obtaining its next event. The current Rust CLI exposes these
launch/protocol building blocks but still ships no subscription runtime command.

A manifest can report only static implementation maturity. Discovery always
reports `configured`, `connected`, and `live_verified` as false. Those are
runtime facts requiring separate evidence; installing a manifest cannot claim a
live provider connection. The reference inventory reports Google Workspace
Events design material with no Rust provider, cloud topic, pull
subscription, application-default credentials, or live end-to-end verification.
It reports Discord request/response support separately from the absent Gateway
subscription. Neither entry is advertised as a working inbound backend.

Extension commands are not available in every installation. An interactive
agent requires Herdr; selecting tmux does not provide interactive agent control.
A headless transcript view is not a native interactive harness interface.

## Names, state, and shared workspaces

Commands use a human-readable session `NAME`, made of lowercase letters, digits,
and hyphens. State defaults to `.agentctl` in the current directory. Use
`--registry /absolute/path` to give multiple callers the same registry. The
`--state` spelling is an alias for the session registry; Chat has a separate
state directory and separate `--state` option.

A name identifies one generation of a session. The registry also records its
runtime identity, selected adapter, native conversation when known, and delivery
artifacts. Reusing a name requires stopping its current session first. The tool
refuses a changed or unverified owner rather than treating a matching directory
or pane title as proof that it found the same agent.

Workers run in the directory given by `--cwd` (default: the current directory).
They share its files and credentials. Separate Git worktrees are an operator
choice when simultaneous edits need isolation; agent control does not require
creating worktrees.

## Start and delegate

```sh
agentctl start reviewer --harness codex --cwd /work/project \
  --brief 'Review the current changes and report actionable findings'
agentctl start implementer --harness claude --cwd /work/project \
  --file /tmp/implementation-task.txt
agentctl start gateway-worker --harness codex --cwd /work/project \
  --env META_CODEX_AI_GATEWAY=azure-codex-cyber:openai \
  --brief 'Use the configured gateway for this task'
agentctl list
agentctl status reviewer
agentctl send reviewer 'Focus on cancellation and restart behavior'
```

`--model` selects an accessible model; omission preserves the harness default.
`--harness-arg=ARG` passes a literal argument to an interactive harness and may
be repeated. `--resume SESSION` resumes an explicitly identified conversation.
Use `--workspace-id` to choose an exact Herdr workspace.

Interactive Herdr starts also accept repeatable `--env KEY=VALUE`. Each entry is
passed as one literal argument to Herdr when it creates the tab, before the
harness starts; spaces, shell characters, additional `=` characters, Unicode,
and an empty value are not expanded or rewritten. Names must match
`[A-Za-z_][A-Za-z0-9_]*`, and neither names nor values may contain NUL. This
option does not apply to headless workers.

Environment values are launch-only input. `agentctl` does not write them to the
session registry or return them from `status` or `list`. They still become part
of the launched process environment and are not a credential vault; use the
host's ordinary credential facilities for secrets. If a launch using `--env`
fails, its retained status record uses a generic diagnostic so terminal-control
errors cannot copy a value into later status output; the immediate command still
reports the launch failure to its caller.

### Adopt an existing Herdr agent

An agent that is already running in Herdr can join the same named registry
without being restarted or renamed:

```sh
herdr pane list
herdr pane get w1:p2
herdr workspace get w1
agentctl adopt reviewer --pane w1:p2 --workspace project \
  --cwd /work/project --harness codex
```

Use the pane ID, working directory, harness, workspace ID, and workspace label
reported by those read-only Herdr commands; do not infer them from a tab title.
`--pane`, `--workspace`, `--cwd`, and `--harness` are required identity
assertions. Adoption resolves the workspace label, canonicalizes the directory,
requires a live matching harness, pins the exact pane and tab, and refuses a
pane or native session already registered under another name. A pane title or
tab label is never enough. If Herdr already reports a native conversation ID,
the record and durable queue retain it automatically. `--session ID` may be
supplied only to assert an ID already reported by that exact live pane; it is not
a way to guess or overwrite native identity.

If `pane get` reports no native session, omit `--session`; the adopted queue
stays bound to the exact pane. A separately recovered ID can later enable native
goal inspection with `agentctl bind-session NAME ID`. That explicit fallback
does not rewrite the queue target and is refused if another registered record or
live pane already reports the same harness-local ID.

The saved adapter is `herdr-foreign`. Named `send`, `read`, `wait`, `goal`,
`bind-session`, `status`, `list`, pause/resume, attach, and durable drain then use
the same interface as a started interactive session. The registry owns those
control artifacts, not the adopted process. Consequently, `agentctl stop NAME`
revalidates the target, saves a final terminal snapshot, unregisters it, and
archives its queue without closing the pane, tab, or process. Stop the foreign
runtime through the authority that created it. If the recorded live identity can
no longer be verified, `stop` refuses and leaves the registration in place.

`send` accepts literal text or `--file`. In interactive mode, the instruction is
persisted before submission. The manager waits for native readiness, records an
in-flight barrier, and asks Herdr to submit the text with paste plus Enter. It
requires evidence of a subsequent working state to confirm acceptance.

The default readiness wait is 900 seconds. For a responsive coordinator loop:

```sh
agentctl send reviewer --file /tmp/follow-up.txt --ready-timeout 0
agentctl drain reviewer --ready-timeout 0
```

Interactive timeout arguments accept finite seconds no greater than 31,536,000
(one year). Readiness waits may be zero; working confirmation must be positive.
The startup deadline must be positive and at most 300 seconds. The count options
`--max-attempts` and `--lines` must be between 1 and 1,000,000.

Known-unsubmitted work remains queued. Uncertain submission is quarantined and
is not automatically injected again. `--message-id` gives an interactive
request a caller-selected identity; duplicate IDs are rejected, not interpreted
as permission to execute again. Inspect the saved artifact before deciding
whether a new instruction is needed.

The interactive queue has four durable directories:

| Directory | Meaning | Recovery |
| --- | --- | --- |
| `inbox/` | The request is safely pending and has not crossed the submission barrier | Run `agentctl drain NAME` against the same registry and queue |
| `inflight/` | Submission may have begun; a crash here leaves acceptance uncertain | Inspect the harness; restart quarantines it without replay |
| `failed/` | Quarantined input, with an error record distinguishing invalid input from `possibly_submitted` | Inspect the error and conversation; never automatically replay uncertain work |
| `processed/` | The target's working transition confirmed submission | Await the agent's result; this is not proof of task completion |

`send` reports `outcome: pending` with exit 75 for safely queued work and
`outcome: possibly_submitted` with exit 76 for uncertain delivery, including the
request ID and artifact path. Preserve that ID and use `drain` for pending
work. Sending the same task under a new ID creates a second request and can
duplicate work; it is not a recovery operation.

## Observe and take over

```sh
agentctl read reviewer --lines 100
agentctl wait reviewer --timeout 60
agentctl pause reviewer
agentctl attach reviewer
agentctl resume reviewer
```

`read` returns a terminal snapshot for an interactive agent. It does not claim
that the snapshot is the final answer. `wait` observes readiness, which can occur
between steps of an active goal. Neither readiness nor successful delivery
proves completion of the task.

`pause` blocks automated input through the session manager; it lets an active
turn finish. `attach` focuses the terminal and does not itself transfer input
ownership. Pause before typing directly, coordinate with any separate input
producer, and clear unfinished composer text before `resume`. The native
harness remains usable directly in Herdr.

## Goals and native conversation identity

```sh
agentctl goal reviewer 'Complete the cancellation review'
agentctl goal reviewer
agentctl bind-session reviewer NATIVE_SESSION_ID
```

Setting a goal submits a native `/goal` command for Codex or an ordinary goal
instruction for other harnesses. It records the requested objective separately
from the native goal result. Native inspection requires a supporting harness,
a known conversation ID, and a working goal RPC interface. If the ID was not
reported automatically, use `bind-session` with the ID reported by that exact
agent. A shared working directory is not sufficient evidence of identity.

`goal`, `bind-session`, and `drain` apply to the native interactive adapter.
Headless turn runners expose their supported operations in `agentctl status`;
they do not acquire native goal RPC support by using Herdr for presentation.

An installation can select its native goal RPC with
`--goal-command-json '["/absolute/path/to/goal-rpc"]'`. `status` and `goal` expose
whether the native state is available; unverified intent is not reported as a
completed native goal.

## Stop and recover

```sh
agentctl stop reviewer
```

Stopping closes only a runtime created and owned by the session manager, then
archives its state. For an adopted `herdr-foreign` record, stopping means safe
unregistration: the manager verifies the recorded identity, saves a terminal
snapshot, archives its record and queue, and leaves the pane, tab, and process
running.
An unavailable terminal server is not proof that an agent died. A lost terminal
view is not proof that a headless runner stopped. Preserve the registry and
inspect `status` when ownership checks refuse an operation.

Interactive prompt delivery distinguishes pending input from uncertain input.
Do not delete a queue or resubmit an uncertain request simply because no final
answer is visible. Check the native terminal, conversation, and durable artifact.
A generation change prevents old work from being silently redirected to a new
worker with the same name.

Most commands print JSON; `read` prints text and documentation commands print
prose. Successful operations exit 0. Pending interactive work exits 75 and
uncertain submission exits 76; failures include a diagnostic. Read the reported
outcome and saved artifact instead of treating every nonzero exit as a request
to submit again.

## Headless worker extension

When listed by `agentctl capabilities`, headless workers use the same names and
registry as interactive sessions:

```sh
agentctl start worker --mode headless --backend tmux --harness codex \
  --cwd /work/project --brief 'Investigate the failing tests'
agentctl send worker 'Summarize the remaining failures'
agentctl read worker --output last
agentctl read worker --output since_turn --since-turn 2
```

The long-lived runner invokes structured harness turns and retains the native
conversation ID between turns. The presentation terminal displays status and
transcripts. Pause automated input before direct intervention. `reset` clears
an idle worker's conversation context; `repair` recreates a missing presentation
for a live runner. `migrate --backend herdr|tmux` moves an idle worker while
preserving its conversation and queue, subject to supported mode transitions.
The source and destination must retain compatible harness permissions.

The worker inbox marks claimed turns before execution. After an interruption,
inspect uncertain turn state and the native conversation rather than assuming
that rerunning a command is harmless. Stop and migration operations verify
ownership and preserve recoverable state on failure.

## Chat and MCP extensions

`agentctl chat quickstart` and `agentctl chat userguide` describe the Chat
extension. It accepts authorized messages into durable state and reacts with
🤖 by default, before waiting for the coordinator to become ready. A reaction
means the bridge ingested the message; the final answer arrives separately.
Configure another emoji with `ack_reaction`, or disable reactions with null or
an empty string. Reaction failures remain visible and retryable without aborting
prompt delivery or replies.

The Chat bridge is a separate, long-running process. It can target one coordinator
without any worker team. It requires an interactive coordinator in Herdr and
uses terminal input; a command or socket transport changes Google Chat access, not the
harness connection. By default, the coordinator brackets its final answer with
unique tags from the prompt. The bridge waits on a Herdr subscription, captures
that block, and posts it durably; no reply file or CLI call is needed from the
agent. Explicit file replies remain available with `reply_mode: "file"` and for
recovery. Retained terminal history is bounded, so capture failures remain
visible for inspection. The bridge does not supervise the coordinator's process.

For an operator-created Herdr tab, `agentctl chat launch --config chat.json`
provides the supervised exception to that separate-process model: it discovers
the current pane and workspace, runs the native coordinator there, and owns a
bridge child for exactly that coordinator's lifetime. The inherited
`HERDR_WORKSPACE_ID` also keeps the coordinator's default `agentctl start`
subagents in the same Herdr workspace. The reusable launch config contains Chat
authority and transport settings; the command supplies the target identity.

The built-in Chat transport polls the public REST API. An optional
`event_command` connects an operator-supplied event stream; messages then wake
the bridge immediately, while ACKs, Herdr prompt delivery, final replies, and
recovery scans run independently. A durable cursor and message IDs allow safe
replay after reconnecting. `run --reconcile-interval` controls REST
reconciliation in streaming mode (default: 300 seconds); the local durable-state
recovery pass remains fixed at five minutes. Without an event adapter,
`run --interval` controls the delay after each completed polling cycle
(0.1–86400 seconds; default: 3600 seconds). Failure backoff never polls more often
than the configured interval and grows toward a one-day ceiling after long-interval
failures. The Chat user guide documents the adapter protocol.

One `run` or `tick` process owns each Chat state directory. Stop `run` before
using `tick` for manual recovery. Explicit `chat reply` submissions remain
available while it runs; a private local notification wakes the streaming
runner immediately after the submission is durable, and the unconditional
five-minute recovery pass covers a missed notification or restart.

Thread replies include a command hint for the nearest ten prior messages:
`agentctl chat context --state DIR --request KEY_OR_UNIQUE_HEX_PREFIX --limit 10`.
It reads one page from the request's exact thread and time cutoff, prints it
chronologically, and supplies a cursor for older context. This command needs
Chat access and saved bridge state, but no live Herdr pane.

`agentctl mcp --registry /absolute/path` serves session operations over stdio
when the MCP extension is available. The coordinator can use either CLI or MCP;
workers do not need individual Chat connections. Its `agent_adopt` tool requires
the same `name`, `pane`, `workspace`, `cwd`, and `harness` assertions and accepts
the same optional already-reported `session` assertion as the CLI.

## Compatibility entry points

The agent-control package supplies `herdr-agent` as a compatibility entry point.
Installations with worker and Chat extensions also supply `herdr-subagents` and
`herdr-chat`. Prefer the single `agentctl` command for new integrations:

| Existing entry point | Unified operation |
| --- | --- |
| `herdr-agent start NAME` | `agentctl start NAME` |
| `herdr-agent send --name NAME TEXT` | `agentctl send NAME TEXT` |
| `herdr-agent status --name NAME` | `agentctl status NAME` |
| `herdr-subagents up NAME ...` | `agentctl start NAME --mode headless ...` |
| `herdr-subagents send NAME ...` | `agentctl send NAME ...` |
| `herdr-chat COMMAND ...` | `agentctl chat COMMAND ...` |

Compatibility commands retain their state-directory defaults: `herdr-agent`
uses `.herdr-agents` for managed sessions and `.herdr-agent` for an explicit
delivery queue; `herdr-chat` uses `.herdr-chat`. The worker compatibility command
uses `HERDR_SUBAGENTS_HOME`, or `$XDG_STATE_HOME/herdr-agent/foreign` (default:
`~/.local/state/herdr-agent/foreign`).

Use `agentctl --registry .herdr-agents` to continue a managed interactive
registry. An independent worker-compatibility registry retains its own command
and environment until those workers are stopped and started through the unified
interface. Creating a new registry does not adopt an agent by matching its name;
use `agentctl adopt` with the explicit live identity assertions above.
Low-level compatibility arguments remain discoverable with each entry point's
`--help`.

Use `agentctl COMMAND --help` for argument descriptions, defaults, units, and
examples. These installed documentation commands are the operator reference;
they do not require access to a source checkout or a web manual.

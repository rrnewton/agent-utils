# headless-coordinator: running the coordinator agent without a TUI

Status: design (milestone M0 of `#565 headless-coordinator`); nothing here is implemented.
Written 2026-10-26 at the owner's request. Harness behaviour marked **verified** was observed on
this host the same day with Claude Code 2.1.310, the Codex CLI and the Muse CLI installed here
(section 3 and the appendix give the commands); everything else is cited or labelled as
inference.

## 0. Summary

Today the coordinator is an interactive Claude Code TUI in a Herdr pane. Every program that talks
to it types into its composer and reads its screen. The chat bridge is the largest such program.
That one decision causes most of the coordinator's delivery problems. Typing is exclusive and
slow, so owner messages queued behind agent messages for up to 56 minutes (`#556
chat-delivery-latency`). Proving a submission means reading every agent's pane. Replies are
scraped between marker lines. The bridge depends on how Claude Code draws its composer, shell
mode and full-screen renderer, and that renderer folds tool output in a way that looks like
scrollback jumping. Claude Code channels would replace the typing, but they need Anthropic
authentication and are not offered through cloud providers
([channels scoping note](../2026-10-26-claude-channels-bridge.md)). They also deliver only on the
next turn.

This note proposes an optional **headless mode**, built from small parts in the suite's usual
style:

1. **Input: the queue agentctl already has.** The headless wrapper is one more drainer of the
   `agent-queue` directory (`inbox/`, `inbox.v2/`, `inflight/`, `processed/`, `failed/`,
   `cancel/`, `cancelled/`). It hands an item to the harness over the harness's structured
   protocol instead of typing it into a composer. Senders do not change: `agentctl send`, the
   inbox deliverer, agent-teams and the chat bridge already write this queue.
2. **Output: a typed JSONL event log** per agent (turns, assistant text, tool activity, input
   acknowledgements, compaction, status). Events **describe**; they never **act**. Effects such
   as chat replies and messages to other agents keep going through the existing idempotent
   commands (`agentctl chat reply`, `agentctl send`), so a replayed log cannot repeat an effect.
3. **One wrapper per harness**, each a small executable that meets one contract (section 6).
   Claude Code comes first: a persistent `claude -p --input-format stream-json --output-format
   stream-json` process, which is **verified** to accept input in the middle of a turn and to
   acknowledge each input by the caller's own ID. Codex (`app-server`, or `exec resume`) and Muse
   (`muse serve`, or `muse exec --session-id`) follow.
4. **`agentctl tail NAME`**, a generic read-only follower of any headless agent's event log, so
   a headless agent can still have a Herdr tab. Closing the tab never touches the agent.
5. **The chat bridge** delivers to a headless coordinator by enqueueing, takes the harness
   acknowledgement as proof of delivery, and gets replies only through `agentctl chat reply`.
   Screen capture is switched off for that target.

Milestones: M1 event log and `tail`; M2 the Claude wrapper; M3 bridge integration; M4 a shadow
day; M5 cutover, with the interactive coordinator kept as the rollback (section 13). An optional
**liaison** reader (section 12) is designed in from the start: it reads the same records as the
coordinator and never relays the coordinator's summaries.

## 1. Why headless

Problems the TUI coordinator has today, each recorded in this repository:

| Problem | Where recorded | Headless effect |
|---|---|---|
| One typist at a time; owner chat waited behind agent sends for up to 56 min | `#556 chat-delivery-latency` | input is a pipe write; no lifecycle lock spanning a readiness wait |
| Submission proof reads the target and every peer pane (1-2.5 min per delivery on a loaded terminal server) | `#556` | the harness acknowledges each input by ID (section 3.1) |
| Composer, shell-mode and scroll-pill screen dependencies | `#559 claude-shell-mode-prompt`, `#397` | none: no screen is read |
| Replies scraped between marker lines; a reply printed in thinking, or redrawn away, is lost | chat user guide, `--offer-reply-command` | replies only through `agentctl chat reply` (already exists) |
| Full-screen renderer folds tool output; readers see "scrollback jumps" | coordinator observation 2026-10-26 | `tail` renders the event log, not a screen |
| Channels unavailable with cloud-provider authentication; next-turn only | [channels note](../2026-10-26-claude-channels-bridge.md) | stream-json needs no channel feature and delivers mid-turn |

What is given up: the owner can no longer type into the coordinator's own TUI. Section 7.3 keeps
two ways to talk to a headless agent directly, and a takeover path back to the TUI.

## 2. What exists today (verified in this tree)

- **`agent-queue`** (`rs/agent-queue/src/lib.rs`): durable per-agent prompt queues with one
  identifier per phase, an at-most-once barrier (`inflight/` plus `possibly_submitted`) before
  typing, crash recovery that never replays automatically except a provably untyped item,
  tombstones and expiry in the `inbox.v2/` lane (`#506 agent-queue`, inventory in
  [2026-10-25-agent-queue-lane-inventory.md](../2026-10-25-agent-queue-lane-inventory.md)).
- **`agentctl inbox`**: coalesced, priority-ordered notices to a coordinator, delivered as one
  batch, retried idempotently per batch.
- **The Python headless worker extension** (`py/agentctl/foreign/agent_runner.py`): a per-agent
  turn runner. It blocks on the agent's inbox, runs one `codex exec resume`, `agy --print` or
  `muse exec --session-id` subprocess per turn, appends the JSONL events to `transcript.log`,
  publishes `last-message.txt`, and writes a `===TURN-DONE <seq> rc=<n>===` sentinel. A Herdr
  tab tails the transcript. It supports Codex, AGY and Muse, **not Claude Code**. The Rust
  edition records such workers but "does not drive turn runners"
  (`rs/agentctl/src/subagents/turn_runner.rs`).
- **The chat bridge** separates coordinator delivery behind the `CoordinatorDelivery` trait
  (`rs/agentctl/src/chat_runtime.rs`: `message_state`, `submit`, `drain`, screen access). It keeps
  durable request records (one JSON file per admitted request, with phase, delivery and reply
  ordinals). `agentctl chat reply --request KEY --reply-id ID --file F` stores a reply and wakes
  the service, with no screen involved.
- **agentcloud sessions** are already headless: `agentcloudctl notify` delivers mid-turn with an
  idempotency key, and `interrupt`, `wait`, `history` and `tail` exist. The headless contract
  below should be satisfiable by that adapter too.

The new work is narrow: a Claude driver, a persistent-process mode, a typed event log, `tail`,
and a headless implementation of `CoordinatorDelivery`.

## 3. Harness capabilities

### 3.1 Claude Code (verified, 2.1.310, through a cloud-provider gateway)

| Capability | Result | How observed (appendix) |
|---|---|---|
| `-p --output-format stream-json --verbose` | `system/init`, `assistant`, `user` (tool results), `result` with usage and `total_cost_usd` | E1 |
| `--session-id UUID` then `--resume UUID` | same session; recall across processes | E2 |
| Persistent process, `--input-format stream-json` | one process, many turns; `result` per turn | E3 |
| **Caller-supplied input ID** | the `uuid` we send comes back as `command_lifecycle` `queued` → `started` → `completed` (or `cancelled`) with `command_uuid` equal to it | E3, E5 |
| **Mid-turn input** | a message sent while a tool ran was `queued` at 7.06 s and `started` at 7.70 s, right after the tool result, inside the same turn; one `result` answered both | E3 |
| Replay acknowledgement | `--replay-user-messages` re-emits each input with `isReplay: true` | E3 |
| Durable receiver-side record | the session transcript records the input's `source_uuid` and a `queue-operation` (`absorbed_mid_turn`) | E3 transcript |
| Interrupt | `control_request {subtype: interrupt}` → `control_response`; queued input `cancelled`; turn ends `error_during_execution`; next input runs | E5 |
| Compaction | `/compact` as an ordinary input: `status: compacting` → `compact_boundary {trigger, pre_tokens, post_tokens}`; recall survived | E5 |
| Permission denial | a deny rule yields a tool error and `result.permission_denials[]` | E5 |
| Turns the harness starts itself | a finished background command produced `task_notification` and a new turn with no input | E3 |
| Hooks | user and project hooks run in `-p` (a PreToolUse hook refused a foreground `sleep`) | E3 |
| Prompt cache | persistent process, turn 2: 77 140 tokens read from cache, 622 written; per-turn `--resume` process: ~30 000 tokens written again each turn | E2, E3 |

The `init` event also advertises capabilities including `msg_lifecycle_v1`,
`interrupt_cancel_queued_v1` and `interrupt_send_now_v1`. Only the first two were exercised here.

Two cautions from the same runs. First, on this host the launcher's sandbox approved a `touch`
even under `--permission-mode dontAsk` with no allow rule; only an explicit deny rule refused it.
The permission policy must therefore be tested on the target host and must include deny rules,
not only an allowlist (section 9.4). Second, `total_cost_usd` is cumulative for the session and
process, as the public docs state for resumed sessions.

The public docs recommend streaming input as the "preferred way" to use the Agent SDK. That is
the same protocol the SDK drives
([streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)). They also
recommend `--bare` for scripted calls
([headless](https://code.claude.com/docs/en/headless)). A coordinator needs its `CLAUDE.md`,
skills and hooks, so it should **not** run bare. The wrapper names what it loads instead.

**Agent SDK or CLI?** The SDK is a library around the same stream-json child process. A Rust
wrapper that speaks stream-json directly avoids a Node or Python runtime and keeps the suite's
language. Keep the SDK as the reference for message shapes.

### 3.2 Codex (verified: `exec`; from schema: `app-server`)

- `codex exec --json --skip-git-repo-check PROMPT` → `thread.started {thread_id}`,
  `turn.started`, `item.completed {agent_message}`, `turn.completed {usage}`. `codex exec --json
  resume THREAD PROMPT` kept the thread ID and recalled the first turn, with 23 552 of 31 162
  input tokens cached (E6).
- `codex app-server` is a JSON-RPC protocol. Its generated schema (`codex app-server
  generate-json-schema`) includes the client requests `thread/start`, `thread/resume`,
  `turn/start`, `turn/steer`, `turn/interrupt` and `thread/compact/start`, and the notifications
  `turn/started`, `turn/completed`, `item/*`, `thread/compacted`, `thread/queue/changed` and
  `thread/tokenUsage/updated`. `turn/steer` is the mid-turn input. It was **not** exercised live
  here.
- `codex queue --thread T --message TEXT` queues a message for an existing session.

### 3.3 Muse (verified: `exec`; from schema: `serve`)

- `muse exec --json --session-id UUID -- PROMPT`, run twice with the same UUID, recalled the first
  turn (E7). Its JSONL is an event-sourced record stream: `schema_version`, `id`, `stream {kind,
  id}`, `sequence`, `recorded_at`, `record_type`, `durability`, `causation_id`, `payload_type`
  (`turn.input.user`, `run.output.delta`, `run.usage`, `run.terminal.completed`, `task.lifecycle.*`).
  This is a good model for our own envelope (section 5.2).
- `muse serve` hosts sessions over stdio using its own wire protocol. The schema it exports (`muse
  schema generate-json-schema`) includes `session/start`, `session/resume`, `turn/start`,
  `turn/steer`, `turn/interrupt`, `turn/unqueue` and `session/compact`. It was not exercised live.
- `muse session-message send --target SESSION` sends cross-session messages.

### 3.4 Summary

| | persistent process | mid-turn input | input ack by caller ID | interrupt | compact | resume |
|---|---|---|---|---|---|---|
| Claude Code stream-json | yes (verified) | yes (verified) | yes (verified) | yes (verified) | yes (verified) | yes (verified) |
| Codex app-server | yes (schema) | `turn/steer` (schema) | to verify in M2b | yes (schema) | yes (schema) | yes (schema) |
| Codex exec resume | per turn | no | no (turn-level only) | kill | no | yes (verified) |
| Muse serve | yes (schema) | `turn/steer` (schema) | `command_id` / `causation_id` (to verify) | yes (schema) | yes (schema) | yes (schema) |
| Muse exec | per turn | no | `command_id` per run (verified) | kill | thresholds | yes (verified) |
| agentcloud | hosted | `notify` (documented) | idempotency key (documented) | `interrupt` | hosted | hosted |

## 4. Lessons from an earlier prototype

The owner built a private prototype stack in another orchestrator in July and August 2026. It
added a per-turn headless CLI backend (`claude -p --resume`, `codex exec resume --json`) and a
daemon-backed backend beside the TUI one. It was never landed, and that codebase has since
standardized on the TUI. The private report holds the details. These lessons carry over:

1. **Declare the delivery contract and test it.** Backends that answer "send" differently (at
   acceptance versus after the whole turn) surprised the orchestrator: one blocked for 120 s on a
   send documented to return immediately. The fix was a declared contract per backend (send
   boundary, send deadline, read retention, whether reads see in-flight output), checked by one
   assertion table with a negative test per assertion. Section 6.3 adopts it.
2. **Per-turn processes stream nothing.** Only the final text surfaced, so the first output came
   at the end of the turn (18 s in its measurements), and a read during a turn returned nothing.
   A coordinator needs the persistent process.
3. **"Absent from this build" is not "broken".** Two binaries of different vintage served agents
   at once and accepted different backends. A doctor that keys on the executing image and says
   which is which saves hours. Pinned agentctl binaries have the same hazard here, so
   `agentctl capabilities` must report headless support per binary.
4. **A dead backend is `unreachable`, not `busy`,** and a still-starting agent's empty output is
   truthful, not an error.
5. **Say when a reader fell behind retention** instead of silently skipping (section 7.2).
6. **Failover between backends does not move a conversation.** Each backend owns its own session
   store. Rollback from headless to the TUI works only because both drive the same harness
   session (section 7.3).

## 5. Protocol

### 5.1 Inbound: queue items

The wrapper drains the agent's existing queue directory under the rules of `agent-queue`. It
takes `inbox/` and `inbox.v2/` together in queue order, and checks tombstones and `not_after`
after the move to `inflight/` and before handing an item over. Headless adds fields; readers
ignore unknown fields, as the format requires.

| Field | Type | Meaning |
|---|---|---|
| `text` | string, required | the input, as today |
| `id` | queue identifier | as today; becomes the harness input ID (Claude: the stream-json `uuid`, which must be a UUID, so a non-UUID ID maps to a UUIDv5 of it, recorded in the item) |
| `from` | string | sender: an agent name, `chat:<request key>`, `inbox`, `owner`, `timer` |
| `priority` | `urgent` \| `normal` \| `low` | delivery order within the queue; absent is `normal` |
| `deliver` | `now` \| `turn-boundary` | `now` hands the item over mid-turn if the harness supports it; `turn-boundary` waits until the current turn ends |
| `reply_route` | object | for chat requests: `{request, reply_id}`, so the coordinator knows how to answer |

Items that use `priority` or `deliver` go to `inbox.v2/`, so drainers that predate the lanes
never see them (the rule in the team-messaging design, section 12.4).

**Order.** `urgent` before `normal` before `low`, then `queued_at`, then name. Within a priority
the order is FIFO, and the harness preserves it (Claude processes queued input in order, E3/E5).

**Default policy.** Owner chat requests are `urgent` + `now`: the owner is answered between tool
calls, not after the turn. Agent reports (`agentctl inbox` batches, child-done notices) are
`normal` + `turn-boundary`, so a burst of worker notices cannot derail the coordinator mid-step.
Timers are `low`. The coordinator can always read its queue (`agentctl status`) to see what is
waiting.

**Exactly-once, end to end.** The queue gives at-most-once into the harness. Headless adds proof
from the receiver:

1. Move the item to `inflight/`, stamp `possibly_submitted` and `harness_input_id`, fsync.
2. Write the input line to the harness's stdin and stamp `written_at`.
3. On the harness acknowledgement for that ID (Claude: `command_lifecycle state=queued`), move the
   item to `processed/`, stamping `acked_at` and later `started_at` / `completed_at` from the
   following lifecycle events.
4. **Recovery** after a wrapper crash. An inflight item without `written_at` was never written,
   so it returns to its lane. An item with `written_at` but no ack is looked up in the harness's
   own session record (Claude's transcript records `source_uuid`): found means `processed/` with
   `recovered: transcript`, and absent means `failed/` with `possibly_submitted`, as today. Nothing
   is replayed on a guess.

The receiver-side record is what makes the claim exact. A harness without one (per-turn `exec`
modes) gets the queue's existing at-most-once and nothing stronger, and its contract says so.

**Cancel and expiry.** A tombstone or an expired `not_after` found after the move to `inflight/`
sets the item aside untyped, as today. For an item already written but not yet `started`, the
wrapper may send the harness's cancel-queued control (Claude advertises
`interrupt_cancel_queued_v1`); the receipt says whether it was in time.

**Interrupt** is not a queue item. It is a control request the wrapper accepts on its control
socket (section 6.1), from `agentctl interrupt NAME`.

### 5.2 Outbound: the event log

One append-only JSONL file per wrapper generation:

```text
<registry>/<agent>/events/<generation>.jsonl   0600, directory 0700
<registry>/<agent>/events/raw-<generation>.jsonl   the harness's own stream, verbatim, for forensics
<registry>/<agent>/events/index.json   {generation: {first_seq, last_seq, started_at, ended_at}}
```

Envelope, one object per line, written with one `write(2)` and fsynced at turn boundaries and
after every `input.*` event:

```json
{"v": 1, "seq": 4182, "id": "evt-...", "ts": 1793059038226, "agent": "coordinator",
 "generation": 7, "session": "<harness session id>", "turn": 311,
 "causation": "<queue item id or null>", "type": "tool.started",
 "data": {"tool": "Bash", "summary": "git fetch origin", "tool_use_id": "toolu_..."}}
```

- `seq` increases by one per event for the agent across generations. A new generation reads the
  last `seq` from `index.json`, so `seq` is a stable address for links (section 12). `ts` is Unix
  milliseconds.
- `causation` links an event to the input that caused it, as Muse's `causation_id` does.

Event types (v1):

| Type | Data | Notes |
|---|---|---|
| `lifecycle.started` / `lifecycle.stopped` | wrapper and harness versions, argv digest, permission profile, exit cause | one per generation |
| `input.accepted` | `queue_id`, `harness_input_id`, `from`, `priority`, `deliver` | the harness acknowledged the input |
| `input.started` / `input.completed` / `input.cancelled` | `queue_id`, `mid_turn: bool` | from the harness lifecycle |
| `turn.started` / `turn.completed` | outcome (`completed`, `interrupted`, `error`), usage, cost delta, duration | `turn.started` also for turns the harness starts itself (`cause: background-task`, `wakeup`) |
| `assistant.text` | text, final or partial (`partial: true` only when streaming deltas are enabled) | the coordinator's prose, verbatim |
| `tool.started` / `tool.completed` | tool, bounded summary (first 200 bytes of the command or path), `ok`, output bytes | never the full output (security, size) |
| `permission.denied` | tool, summary, rule | from the harness's denial list |
| `compact.started` / `compact.completed` | trigger, pre and post tokens | section 9.5 |
| `status` | `starting`, `idle`, `busy`, `blocked`, `unreachable`, `stopped` | derived; replaces screen inference |
| `gap` | `from_seq`, `to_seq`, reason | written by a reader's renderer, never by the wrapper |
| `error` | class, message | protocol or harness errors |

Thinking blocks are not logged (Claude returns them signed and empty in stream-json anyway).

**Events describe, commands act.** The coordinator still replies to chat with `agentctl chat
reply` and messages agents with `agentctl send`, through its shell tool. Those commands are
already durable and idempotent per reply ordinal or message ID. The log records them as
`tool.started` / `tool.completed` like any other tool. The plan's `chat_reply` and
`send_to_agent` events are therefore **derived views** for readers, built by `tail` from those
tool events, and nothing consumes them to cause an effect. Re-reading or replaying a log can
never post a reply twice. Section 15 asks the owner whether a later version should add an effect
channel (an MCP tool exposed by the wrapper) instead.

**Retention.** Keep the event logs of the last N generations (default 30) or 512 MiB, whichever
is smaller. Raw logs are kept for 7 days. Pruning updates `index.json`, so a reader whose cursor
is older sees a `gap`.

## 6. The wrapper

### 6.1 Contract

Each harness gets one executable, `agentctl-harness-<harness>` (first `agentctl-harness-claude`).
It is found the way chat plugins are found (manifest only, nothing executed during discovery),
and agentctl launches it:

```text
agentctl-harness-claude run --registry DIR --agent NAME --generation N
    --cwd DIR --session UUID [--resume] --profile FILE [-- HARNESS_ARGS...]
agentctl-harness-claude capabilities        # prints the contract declaration (6.3) as JSON
agentctl-harness-claude conformance --fake  # runs the assertion table against a fake harness
```

While it runs, the wrapper:

- launches one harness child in its own process group (and the slot's cgroup when slotted),
  pinned by kernel process generation as agentctl already does for panes;
- drains the queue (5.1), holding the queue's delivery lock only while moving and writing an
  item, never across a turn;
- writes the event log (5.2) and the raw log;
- serves a control socket, `<registry>/<agent>/control.sock` (0600), with `interrupt`,
  `cancel-queued ID`, `status` and `stop`. agentctl's commands use it, and it is never exposed on
  the network;
- on harness exit, records the cause. It restarts the harness with `--resume SESSION` only if the
  profile allows it, with backoff, and otherwise exits and lets systemd decide (section 9.1);
- on SIGTERM, closes the harness's stdin (Claude then cancels a pending permission prompt and
  ends), waits, and writes `lifecycle.stopped`.

### 6.2 What stays out of the wrapper

The wrapper does not choose work or retry inputs on its own, and it does not render anything
(that is `tail`'s job). It holds no chat state (the bridge keeps it) and does not touch the
network beyond what the harness does.

### 6.3 Declared contract and conformance

`capabilities` prints the wrapper's declaration, which agentctl and the bridge read before
choosing a delivery policy:

```json
{"schema": "agentctl-harness-capabilities/v1", "harness": "claude", "harness_version": "2.1.310",
 "persistent": true, "mid_turn_input": true, "input_ack": "by-caller-id",
 "receiver_record": "transcript-source-uuid", "interrupt": true, "cancel_queued": true,
 "compact": "command", "resume": "session-id", "self_started_turns": true,
 "send_boundary": "acceptance", "streams_text": "per-message"}
```

The conformance table, adopted from the earlier prototype's lesson, checks behaviour against the
declaration with a fake harness (a scripted stream-json peer) and, opt-in, against the real one.
Example assertions: send returns at acceptance within the declared deadline; an ack carries the
caller's ID; a mid-turn input starts before the turn's `result`; a crash after write and before
ack recovers through the receiver record; an interrupt cancels queued input; and `status` never
stays `busy` after the harness dies. Every assertion has a negative test that breaks exactly one
property.

### 6.4 Codex and Muse wrappers

- **Codex, M2b**: `app-server` over stdio, using `thread/resume`, `turn/start` and `turn/steer`
  for `deliver: now`. Fallback: the Python extension's `exec resume` loop (per turn, no mid-turn
  input, `input_ack: turn-level`). The contract marks the difference, and the bridge then uses
  `turn-boundary` delivery.
- **Muse, M2c**: `muse serve`, using `session/resume`, `turn/start` and `turn/steer`. Fallback:
  the existing `muse exec --session-id` loop.

Both must pass the same conformance table, relaxed only where their declaration says so.

## 7. Viewing and talking to a headless agent

### 7.1 `agentctl tail NAME`

A read-only follower of one agent's event log. It needs no lock, and it can run as any number of
copies.

```text
agentctl tail NAME [--follow] [--since SEQ | --last N] [--json] [--raw] [--types T,...]
                   [--width COLS] [--at SEQ]
```

- The default render is one line per event, with prose wrapped, tools on one line (`$ git fetch
  origin  ✓ 1.2 s`), input acknowledgements marked with the sender (`← owner (chat, mid-turn)`),
  and turn boundaries ruled with usage and cost.
- `--json` prints events verbatim, for scripts and the liaison (section 12). `--raw` shows the
  harness's own stream.
- `--follow` uses inotify where available and falls back to a 250 ms poll. It follows across
  generations through `index.json`.
- A cursor older than retention prints a `gap` line with the missing range.
- `--at SEQ` prints the event with that `seq` and its turn, which is the target of record links.

`agentctl start NAME --mode headless` (M2) opens a Herdr tab running `agentctl tail NAME
--follow`. That tab is a view: Herdr's agent status for it is unknown, and agentctl reports
status from `status` events instead. `tail` is generic, so it works for any harness that writes
the log. It works for the Python extension's workers too, once their runner writes v1 events
beside `transcript.log`.

### 7.2 Status for watchers

`agentctl status NAME` and `agentctl watch-agents` read the last `status` event and the queue
instead of a screen. That means `idle` after `turn.completed` with no queued input, `busy` during
a turn, and `blocked` never (no prompt can appear under `--permission-prompts none`; a denial is
logged instead). A wrapper whose control socket does not answer within 10 s while its process is
alive is `unreachable`; a dead one is `stopped`. The watcher's "idle without DONE:/BLOCKED:"
rule reads the last `assistant.text` of the turn.

### 7.3 Talking to it directly, and taking over

- `agentctl send NAME TEXT` works unchanged: it enqueues, and the wrapper delivers.
- `agentctl say NAME` (M2) is an interactive line editor in the `tail` tab's split. Each line
  becomes an `urgent` + `now` queue item from `owner`. This replaces typing into the TUI.
- **Takeover**: `agentctl takeover NAME` stops the wrapper cleanly at a turn boundary, then
  starts the interactive TUI in a tab with `claude --resume SESSION`, the same conversation. The
  agent becomes an interactive agentctl agent until `agentctl release-headless NAME` stops the
  TUI and restarts the wrapper with `--resume`. Both directions are logged as lifecycle events.
  This is also the rollback for M5.

## 8. Chat bridge changes (M3)

A new `CoordinatorDelivery` implementation, `HeadlessDelivery`, is selected when the target
agent's record says `mode: headless`:

| Today (pane) | Headless |
|---|---|
| enqueue, lifecycle lock, readiness wait, type, verify on screen | enqueue an `urgent` + `now` item with `reply_route`; done |
| proof of delivery from the composer and peer read-backs | `input.accepted` event for the item's ID (harness ack); `input.started` marks "seen by the model" |
| reply capture from screen and transcript, alias gate | none; replies only through `agentctl chat reply` (the prompt names it, as with `--offer-reply-command` today) |
| reply reminders read panes | a reminder is a `normal` queue item naming the unanswered request |
| receipt reaction on verified submission | on `input.accepted` |

`message_state` maps queue phases plus the new stamps (`acked_at`, `started_at`). A request whose
item lands in `failed/` with `possibly_submitted` becomes `delivery_uncertain` for an operator,
exactly as a pane delivery does today. The latency that remains is the time until the
coordinator's current tool call returns. That is under a second for most tools (E3: 0.64 s from
send to `started`), but it is the whole run of a long foreground command. Headless coordinators
should therefore run long commands (gates of 35-40 min) in the background, as the harness already
encourages (E3: the hook refused a foreground `sleep`).

Channels remain an option for an interactive coordinator where Anthropic authentication is
available. Headless does not need them.

## 9. Operations

### 9.1 Process supervision

- One systemd user unit per headless agent, installed by `agentctl install-units` (which already
  writes units for the bridge and watcher): `Restart=on-failure`, `RestartSec=5`,
  `StartLimitIntervalSec=600`, `StartLimitBurst=5`, `KillMode=mixed`, `TimeoutStopSec=120`. The
  unit runs `agentctl run-headless NAME`, which execs the wrapper with the recorded launch spec.
  `restore` replays the launch spec as it does for interactive agents.
- The wrapper never runs two harness processes for one session. Startup takes an exclusive lock
  on `<registry>/<agent>/headless.lock` and refuses if a previous generation's harness, pinned by
  PID and start time, is still alive.

### 9.2 Restart and resume

On start, the wrapper runs `claude -p --input-format stream-json ... --resume SESSION` when the
record has a session, and `--session-id NEW` otherwise. It then recovers `inflight/` (5.1) before
draining. A resumed Claude session leaves an interrupted turn as it was unless
`CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` (public docs). The wrapper does not set it. Instead it
enqueues one `normal` item, "you were restarted at T; last event seq S", so the coordinator
re-reads its records.

### 9.3 Crash recovery

| Crash point | Effect | Recovery |
|---|---|---|
| wrapper dies before writing an item | item in `inflight/` without `written_at` | back to its lane |
| wrapper dies after writing, before ack | `written_at`, no `acked_at` | look up the transcript's `source_uuid`: processed, or failed `possibly_submitted` |
| harness dies mid-turn | `turn.completed outcome=error`; status `unreachable` until restart | restart with `--resume`; restart notice queued |
| event log write fails (disk full) | wrapper stops taking input (the queue keeps it) and exits nonzero | systemd restarts after space returns; no input is lost |

### 9.4 Non-interactive permissions

- `--permission-prompts none`: anything that would prompt is denied and logged as
  `permission.denied`. Nothing can hang waiting for a human.
- An explicit profile file: allow rules for the coordinator's tools (agentctl, wrkslots,
  herdr-run, git read commands, the gate) plus **deny rules** for what must never run, such as
  force pushes, edits to live service units, and `rm -rf` outside slots. E5 showed that on this
  host the sandbox can approve commands without an allow rule, so the deny list is the guard that
  holds.
- The profile is recorded with a digest in `lifecycle.started`, and a change requires the owner's
  approval, like the existing `--trust-workspace` decision.

### 9.5 Long sessions and context

- Auto-compaction stays on. `compact.*` events record every compaction with token counts, so a
  reader can tell what the model's memory lost.
- At `compact.completed`, the wrapper enqueues a `normal` item pointing at the records the
  coordinator should re-read (status ledger, open requests, active slots), in the role `gt prime`
  plays in Gas Town. Records, not memory, carry state across compaction. That is also the
  liaison's premise.
- `/compact` can be issued deliberately (E5) by `agentctl compact NAME` at a quiet moment.

### 9.6 Cost and usage

`turn.completed` carries usage and the cost delta. `agentctl status` shows tokens and cost per
day. The persistent process keeps the prompt cache warm, which per-turn resume cannot (3.1).

## 10. Security

- **Authority is the filesystem**, as today: whoever can write the agent's queue directory (0700,
  owner-only, and agentctl refuses a non-private queue) can instruct the agent. Headless adds no
  listener. The control socket is 0600 in the registry, and the harness talks only over stdio.
- **No secrets in logs.** Tool summaries are bounded and never include environment values. The
  raw log is the harness's own stream at 0600 with short retention. Profiles name environment
  variables, never their values, as `agentctl status` already does.
- **Untrusted text is labelled.** Every item carries `from`, and the wrapper prefixes chat and
  agent-sent text with a one-line provenance header (the same forgery-proof rendering the inbox
  uses), so a message cannot pose as the owner or as a system notice.
- **Permissions** as in 9.4. The coordinator profile is reviewed like code.
- **Tokens and credentials.** The harness keeps its own authentication. The wrapper never reads
  or forwards it, and a `--bare` mode (which changes authentication sources) is not used.

## 11. What the owner sees

| Before | After |
|---|---|
| the coordinator's TUI in the "coord" tab | `agentctl tail coordinator --follow` in the same tab, plus `agentctl say` for typing |
| replies in Slack via screen capture or `chat reply` | replies via `chat reply` only |
| folded tool output, scroll jumps | one line per tool, nothing folded or redrawn |
| `claude agents` shows it busy or idle | `agentctl status coordinator` shows it from events |

## 12. Optional liaison

The owner's concern is a "telephone game" between a liaison that stays responsive to the owner
and a coordinator that does the work. Each hop of paraphrase loses or distorts something. The
design therefore makes the liaison a **second reader of the same primary records**, and never a
relay of the coordinator's summaries.

### 12.1 The primary records

Every record a liaison may cite is append-only or versioned, has a stable address, and is written
by the party that owns the fact:

| Record | Written by | Address | Exists today? |
|---|---|---|---|
| Coordinator event log (5.2) | the wrapper | `events#<seq>`; `agentctl tail coordinator --at SEQ` | new in M1 |
| Request ledger | the chat bridge | request key; `agentctl chat thread`, `chat history`, `chat sent` | yes, as per-request JSON records; M1 adds an append-only `requests.jsonl` journal of admissions, replies and decisions |
| Status ledger | the coordinator, through `agentctl ledger` | `ledger#<seq>`; one entry per change (lane opened, slot, issue, state, blocking question) | informally (a hand-edited JSON file); M1 makes it an append-only journal with a current-state view |
| Handoffs | workers, through wrkslots | slot and generation; `wrkslots read-handoff` | yes |
| Chat history | the provider, mirrored by the bridge | provider permalink plus request key | yes |
| Issues | anyone, through `mb` | `#<number> <slug>` | yes |

A link is the address, rendered with a local command that resolves it: for example
`coordinator events#4182 (agentctl tail coordinator --at 4182)` or a chat permalink. The
coordinator's own replies should carry the same links, which is good practice regardless.

### 12.2 Ground rules (owner, 2026-10-26)

1. **Answers only from records, with links.** Every factual sentence in a liaison reply cites at
   least one record address. A sentence without a source is not sent.
2. **Owner decisions are written verbatim to the request ledger** (`kind: decision`, the owner's
   exact text, the chat permalink, and the request key), not paraphrased. The coordinator
   receives the decision as a queue item that references the ledger entry and quotes the
   original text. It never receives a liaison summary.
3. **Substantive answers come from the coordinator.** That covers plans, judgments, priorities,
   and anything not already recorded.
4. **Everything else gets "the coordinator has this" plus a link** to the request ledger entry.
   The liaison never guesses.

### 12.3 How the rules are enforced, not just requested

- **Write paths.** The liaison has exactly two: `agentctl chat reply` (its own answers, labelled
  as the liaison) and `agentctl ledger decision` (verbatim owner text plus permalink). It cannot
  `agentctl send` to the coordinator, so it cannot inject a summary. Its permission profile
  allows read-only tools over the record paths and those two commands, and denies everything
  else.
- **The bridge routes; the liaison does not.** Every owner message is still delivered to the
  coordinator's queue (5.1), exactly as without a liaison. The liaison receives a copy and may
  answer first if the answer is already in the records. When a decision is recorded, the
  coordinator gets the ledger reference through the bridge, from the owner's original message,
  not from the liaison.
- **No double answers.** The request ledger shows `answered: liaison (records)` with the reply's
  links. The coordinator sees that in the request item and adds to it only if the question needs
  judgment.
- **Lint at the boundary.** The liaison's reply command refuses a reply in which a sentence has
  no record address, unless the whole reply is the fixed "the coordinator has this: <link>"
  form. This is checkable code, not a prompt instruction.
- **Same records, same events.** A2A states this property for task streams: every subscriber
  receives the same events in the same order, and closing one stream does not affect the task
  (section 16). Two readers of one log cannot drift apart the way two narrators can.

### 12.4 When a liaison is worth it

Gas Town has no such role (16.1): its Mayor both faces the owner and dispatches, and patrol
agents take routine monitoring off it. Headless mid-turn delivery removes most of the latency a
liaison would hide, because the coordinator sees an owner message after its current tool call,
not after its turn (E3). A liaison is still useful when the coordinator is deliberately busy for
long stretches (a long foreground operation, or a compaction), and as a cheap always-on answerer
of "what is the state of X" questions from records. Recommendation: build the records in M1
whether or not a liaison follows, since the coordinator, `tail` and post-compaction recovery all
need them, and trial a liaison only if the M4 shadow day shows owner questions waiting.

## 13. Migration plan

| Milestone | Deliverable | Exit criterion |
|---|---|---|
| **M0** (this note) | design, private report on the earlier prototype, issue `#565` | note landed; owner answers to section 15 |
| **M1** event log and `tail` | v1 event schema and a writer library; `agentctl tail`; append-only request and status ledgers; the Python headless extension writes v1 events beside `transcript.log` | `tail --follow` renders a live Python-extension worker; gap reporting tested; ledgers written by the bridge and by `agentctl ledger` |
| **M2** Claude wrapper | `agentctl-harness-claude` with queue draining, receiver-side recovery, control socket, `capabilities`, conformance table with a fake harness; `agentctl start --mode headless`, `say`, `interrupt`, `compact`, `takeover`, `release-headless` | conformance green; a headless reviewer agent runs a real task; crash-injection tests pass (kill at each point in 9.3) |
| **M2b / M2c** Codex and Muse | `app-server` and `serve` wrappers, or the per-turn fallback | the same conformance table, relaxed per declaration |
| **M3** bridge integration | `HeadlessDelivery`; reminders as queue items; screen capture off for headless targets; status from events in `watch-agents` | a second, non-owner coordinator answers chat end to end headless; delivery latency measured (admission to `input.started`) |
| **M4** shadow day | a headless coordinator running beside the interactive one, receiving copies of owner messages with replies suppressed (logged, not posted); compare answers and latencies | a written comparison; no lost or duplicated request across one day; permission denials reviewed |
| **M5** cutover | the bridge targets the headless coordinator; the TUI tab becomes `tail` plus `say`; the rollback (`takeover`) is documented and rehearsed | one week without needing takeover, or a recorded reason for each takeover |

Each milestone lands through the normal gate. Live services change only at M3 (a second
coordinator) and M5, each with the owner's approval and a recorded rollback.

## 14. Risks

| Risk | Mitigation |
|---|---|
| stream-json is a moving interface (2.1.310 today; fields like `command_lifecycle` may change) | wrapper pins a tested harness version range; `capabilities` reports the harness version; conformance runs against the real harness in M2 CI-opt-in |
| a long foreground tool call delays mid-turn input | coordinator guidance to background long commands; `interrupt` for urgent cases; measure in M4 |
| harness-started turns (background tasks, wakeups) interleave with queued input | events carry `cause`; queue ordering is unaffected; `tail` shows it |
| sandbox auto-approval makes the allowlist weaker than it looks | deny rules; profile reviewed; denial log watched in M4 |
| the owner loses the "glance at the TUI" view | `tail` is a better view of the same facts; `takeover` restores the TUI on demand |
| silent context loss at compaction | `compact.*` events; records-first recovery prompt (9.5) |
| two coordinators answering one request during shadow | replies suppressed in M4; the request ledger records which agent answered |
| event log growth | retention by generation and size; raw logs short-lived |
| pinned older binaries cannot drive headless agents | `capabilities` per binary; an interactive-only drainer refuses an agent recorded `mode: headless` (it has no pane) |

## 15. Questions for the owner

1. **Mid-turn policy.** Should owner chat always be `now` (answered between tool calls), or only
   when a message is marked urgent, with ordinary chat at the turn boundary?
2. **Effects through commands only?** v1 keeps chat replies and agent messages as shell commands
   (idempotent today). Do you want a later wrapper-provided MCP tool for replies instead (a
   cleaner tool call in the log, Claude-specific)?
3. **Takeover.** Is `takeover` (stop headless, open the TUI on the same session) the right escape
   hatch, or do you want the TUI and headless to coexist read-only (a TUI attached to a live
   headless session is not supported by Claude Code stream-json as far as checked)?
4. **Liaison.** Build the records in M1 regardless (recommended), and decide on a liaison after
   M4? Which model would a liaison run (cheaper is the point)?
5. **Codex and Muse coordinators.** Should M2b and M2c come before M3, so the bridge supports all
   three from the start, or after the Claude cutover?
6. **Permission profile.** Who signs off the coordinator's allow and deny lists, and should they
   be checked into this repository as an example, with host-specific parts kept private?
7. **Shadow day scope.** Is a day with suppressed replies enough, or do you want the headless
   coordinator to answer a second, low-stakes channel for real first?

## 16. Related work

### 16.1 Gas Town (read at commit `de2ce8efa7aa`, 2026-10-25)

Sources: `docs/overview.md`, `docs/glossary.md`, `internal/templates/roles/mayor.md.tmpl`,
`docs/design/mail-protocol.md`, `docs/design/escalation.md`,
`docs/agent-provider-integration.md`, `docs/design/witness-at-team-lead.md` in
[github.com/steveyegge/gastown](https://github.com/steveyegge/gastown).

- **Roles.** Infrastructure roles are the Mayor ("global coordinator", singleton), the Deacon
  ("background supervisor daemon", singleton), a Witness per project ("polecat lifecycle
  manager"), and a Refinery per project ("merge queue processor"). Worker roles are Polecats
  (persistent identity, ephemeral sessions, Witness-managed), Crew (long-lived, user-managed) and
  Dogs (the Deacon's infrastructure helpers; Boot checks the Deacon every 5 minutes)
  (`overview.md`, `glossary.md`).
- **Who faces the owner.** The Mayor does. The glossary says it initiates convoys, coordinates
  work distribution, and notifies users of important events. Its role prompt says it "is a
  **coordinator**, not a solo coder" whose default response to a user request is `bd create` then
  `gt sling`, and it lists "Work dispatch", "Escalation handling" and "Strategic decisions" as its
  responsibilities, with "routine nudging (Witness handles that)" as **not** its job
  (`mayor.md.tmpl`). The human is the Overseer, the last tier of escalation (agent → Deacon →
  Mayor → Overseer, `escalation.md`).
- **Answer to the owner's question:** no Gas Town role separates owner-facing responsiveness from
  dispatch. The Mayor does both. What Gas Town separates is **monitoring**: the Deacon, Witnesses
  and Boot patrol the workers, so the Mayor is not interrupted by routine health checks. Its
  "Propulsion Principle" (execute hooked work without waiting for confirmation) addresses the
  opposite failure: a coordinator that waits for the owner and stalls the town.
- **Messaging.** `gt mail` creates a persistent bead in the Dolt-backed tracker, so mail survives
  session death. `gt nudge` sends text "directly to an agent's tmux session", appearing as a
  system reminder, and is lost if the session is dead. The rule is "default to `gt nudge`; only
  use `gt mail send` when the message MUST survive the recipient's session death". Each role has
  a mail budget (`mail-protocol.md`). Beads also implement groups, claim-once queues and pub/sub
  channels.
- **Work tracking.** Beads (issues) and convoys (batches of beads that "land" when their issues
  close) are the ledger. The Mayor's prompt calls completions "a permanent ledger of demonstrated
  capability" (`overview.md`, `mayor.md.tmpl`).
- **Transport.** Tier 0 integration is tmux `send-keys` and `capture-pane` ("timing-sensitive and
  has no delivery confirmation"). Non-interactive `exec` modes are used for formulas and Dogs.
  The forward-looking provider contract adds `SendMessage()` and `GetStatus()` to replace
  `send-keys` and screen scraping (`agent-provider-integration.md`). A design (not implemented)
  would move Witness-managed polecats onto Claude Code agent teams (`witness-at-team-lead.md`).
- **Taken here:** records over memory after compaction (`gt prime` ↔ the post-compaction
  prompt in 9.5); patrol separated from the coordinator (already true here: `watch-agents`);
  durable versus ephemeral messages (queue items versus interrupts). **Not taken:** tmux as the
  coordinator's transport.

### 16.2 Protocols and SDKs

- **A2A** (read at [a2aproject/A2A](https://github.com/a2aproject/A2A) commit `98279d623163`,
  `docs/specification.md` sections 3.4-3.5 and `specification/a2a.proto`). Tasks have the states
  `submitted`, `working`, `input-required`, `completed`, `failed`, `canceled`, `rejected` and
  `auth-required`. `contextId` groups tasks and messages into one conversation. Events must be
  delivered in generation order, and several streams per task receive the same events in the
  same order. Taken: ordered, multi-reader event streams (5.2, 12.3). A2A is a network protocol
  between agents. This design stays local and file-based, though the event schema could be
  bridged to A2A task events later.
- **MCP**: the stdio tool protocol harnesses already speak. Relevant here as the possible effect
  channel of question 15.2, and as the basis of Claude Code channels (see the
  [channels note](../2026-10-26-claude-channels-bridge.md)).
- **Claude Agent SDK streaming input**: a persistent session that queues messages, takes
  interrupts and surfaces permission requests; called the preferred mode
  ([docs](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)). The wrapper
  speaks the same stream directly. SDK subagents run inside one harness process and report to
  their parent natively. They suit short delegated tasks; persistent, separately visible workers
  stay with agentctl.
- **Codex app-server**: already compared in
  [common/docs/agentctl/RELATED_WORK.md](../../../common/docs/agentctl/RELATED_WORK.md);
  section 3.2 adds the verified `exec resume` behaviour and the schema's `turn/steer`.

### 16.3 Orchestrators

- **OpenHands** (read at [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands) commit
  `dbe578d5dbac`, `README.md`): the frontend repository now delegates to an Agent Server, "a REST
  API for running multiple agents on a single machine", implemented in its software-agent SDK
  together with "conversations, workspaces, events". A separate automation service "decides when
  work runs", and the agent server "decides what runs". Taken: the split between deciding when to
  run (here: queue policy and the bridge) and running (here: the wrapper).
- **CLI Agent Orchestrator, Happy, Claude Squad**: compared in
  [common/docs/agentctl/RELATED_WORK.md](../../../common/docs/agentctl/RELATED_WORK.md). CLI
  Agent Orchestrator marks a message delivered before the paste, the same choice as agentctl's
  `inflight/` barrier. Headless adds a receiver-side record that turns "possibly submitted" into
  a definite answer.
- **Cross-host messaging systems** (Matrix, NATS, AutoGen, MCP Agent Mail): compared in the
  [team-messaging related work](../2026-10-24-agentctl-team-messaging/RELATED_WORK.md). The
  headless queue is the last hop those designs end at, minus the terminal composer.

## Appendix: experiments (2026-10-26, this host)

All runs used a scratch directory outside any Git working tree and cheap prompts. Commands are
abbreviated. The driver scripts were kept with the private report.

- **E1** `claude -p --model haiku --output-format stream-json --verbose --session-id U "..."`:
  23 `hook_started` and 23 `hook_response` events, then `init` (with `apiKeySource:
  apiKeyHelper`), `assistant`, `result`. The session ID matched U.
- **E2** the same with `--model sonnet`, then `--resume U "What word ..."`: the answer was
  `PLUM-42`. Usage: turn 1 wrote 24 415 cache tokens, and the resumed process wrote 30 363 and
  read 11 915.
- **E3** a persistent process with `--input-format stream-json --replay-user-messages
  --permission-mode dontAsk --allowedTools 'Bash(sleep *)'`. Message 1 asked for `sleep 25; echo
  done-sleeping`. A PreToolUse hook refused the foreground form and the model moved it to the
  background. Message 2, sent at 7.06 s, was `queued` at 7.06 s and `started` at 7.70 s, and the
  model answered it within the same turn (`result` at 8.86 s). The background task's completion
  started a turn by itself (`result` at 35.23 s). The transcript recorded message 2's
  `source_uuid` with `queue-operation remove reason=absorbed_mid_turn`. Turn 2 usage: 622 cache
  tokens written, 77 140 read.
- **E4** `dontAsk` with `--permission-prompts none` and no allow rule for Write or `touch`: both
  ran, approved by the host's sandbox policy.
- **E5** `--disallowedTools 'Bash(touch *)'`: denied, listed in `permission_denials`. Then
  `/compact`: `compact_boundary {trigger: manual, pre_tokens: 37807, post_tokens: 12768}`, and
  recall afterwards was correct. Then a long essay request followed 4 s later by `control_request
  interrupt`: `control_response success`, `[Request interrupted by user]`, the queued input
  `cancelled`, `result error_during_execution`, and the next input answered normally.
- **E6** `codex exec --json --skip-git-repo-check "..."`, then `codex exec --json resume T
  "..."`: the thread ID was stable and the answer was `FIG-17`. Usage: 15 541 input tokens (8 448
  cached), then 31 162 (23 552 cached).
- **E7** `muse exec --json --reasoning-effort low --session-id U -- "..."` twice: the answer was
  `KIWI-9`, and the run ended with `run.terminal.completed`.

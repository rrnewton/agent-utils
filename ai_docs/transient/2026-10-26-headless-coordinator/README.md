# headless-coordinator: running the coordinator agent without a TUI

Status: design (milestone M0 of `#565 headless-coordinator`). Since 2026-10-27, the event log
and `agentctl tail` (M1, `#582 headless-m1-event-log`) and the Claude Code wrapper (M2, `#583
headless-m2-claude-wrapper`) are implemented; see "Implementation status" below for what was
built, what differs from this note, and what remains.
Written 2026-10-26 at the owner's request, revised the same day after an adversarial review.
Harness behaviour marked **verified** was observed on this host that day with Claude Code
2.1.310, the Codex CLI and the Muse CLI installed here (section 3 and the appendix give the
commands and what each run showed). Everything else is cited or labelled as inference or
"to verify".

## Implementation status (2026-10-27)

Built (operator reference: `agentctl userguide`, "Headless Claude agents"):

- The v1 event log and `agentctl tail` as in 5.2 and 7.1, with two additions to the type list:
  `lifecycle.harness_spawned` / `lifecycle.harness` / `lifecycle.harness_exited` (harness process
  and version, separate from the wrapper's generation) and `interrupt.requested`. `--follow` polls
  every 250 ms; inotify is not used.
- The wrapper is built into `agentctl` as `agentctl run-headless NAME`, not a separate
  `agentctl-harness-claude` executable, so pinned single-binary installs carry it, and
  `agentctl capabilities` reports it per binary (lesson 3 of section 4). The record is `adapter:
  harness-wrapper`, `mode: headless`; the session lives in the record's `headless` block. Launch
  profiles are unchanged (both editions still refuse a headless Claude profile); harness options go
  through `--harness-arg`.
- Delivery as in 5.1, with the lock held per transition rather than across the acknowledgement
  wait (the wrapper is the only drainer, so nothing else waits on it). Differences: the input ID
  is a fresh UUID per attempt, not derived from the queue ID, so a reused or returned queue ID can
  never match an older transcript entry; the acknowledgement clock starts after a 120 s startup
  grace; an input queued but left unstarted by an idle harness is quarantined after the same
  30 s, so delivery cannot wedge; stdin is fed by a writer thread. A harness that dies inside a
  turn gets the restart notice too, and the harness identity is kept in `harness.json` and the
  record (agent-teams sender proof needs it). `send --urgent` is
  `urgent` + `now`; plain `send` and every item without fields is `normal` + `turn-boundary`.
  `agentctl drain` on a headless agent wakes the wrapper and waits, so agent-teams delivery
  (enqueue, then `agentctl drain`) works unchanged.
- `interrupt`, `compact` (queues `/compact`), `status`/`list` from the wrapper's control socket,
  `stop` (clean stop, archive), `install-units --headless NAME`, `--permission-prompts none` by
  default, and an example settings file with deny rules in the user guide.

Revised the same day after the owner's first look at the trial's tail tab (`#588
headless-tail-pretty`):

- `tail` is colored per event kind on a terminal, follows the terminal's width, opens each turn
  with a header naming its sender, and renders the agent's prose, inputs and replies as Markdown
  (line breaks kept, soft-wrapped with hanging indents, boxed code, aligned tables). `--plain`
  or `NO_COLOR` keeps the layout without colors.
- **Difference from 5.2:** `tool.completed` now carries a `preview` of the result (first 8 lines,
  200 bytes each, credential-shaped words masked, none for `Read`, withheld after a private-key
  header) so `tail` can fold long output to its first lines. The design kept output out of the
  log entirely; masking is best-effort, so a secret printed in an unrecognised shape can reach
  the log (0600, retained by age). `run-headless --tool-preview-lines 0` turns it off.
- A new `reply.sent` event describes a successful `agentctl chat reply`, `agentctl send` or
  `agent-teams reply`/`send` run through the shell tool, with the text it sent (inline, a
  here-document, or the named file read back). It is the derived `chat_reply` view of 5.2 built
  by the wrapper instead of `tail`, because the 200-byte tool summary cannot hold the text. It
  still only describes; nothing consumes it to act.

Event log rotation (`#590 headless-log-rotation`): a generation's file is closed at 64 MiB and
continued in `<generation>.<n>.jsonl`; retention (90 days, 2 GiB) prunes whole segments at start
and at every rotation, never the one being written. `tail` follows across segments, and its
`--since`, `--last` and `--at` read only the segments they need; `--follow` without a selection
starts with the last 200 events instead of the whole log. A writer that restarts after a crash
right after a rotation takes `seq` from the newest segment that holds an event, not from a
stale index.

Status for watchers (`#589 headless-watch`, 7.2): `watch-agents` reads the wrapper's status and
the newest 512 KiB of the event log: turn age, the tool running, queued inputs, and the last
turn's prose for the verdict rule. A silent log inside a turn is `stalled` from its last event;
input left queued while idle is `stalled` after the dead threshold; a dead or unreachable
wrapper is `dead`; there is no `blocked`. The harness process sample (`/proc`) is taken as for
interactive agents but not yet used for headless tool-blocked findings.

Verified on Claude Code 2.1.310 (the 3.1 "to verify in M2" list): `--input-format stream-json`
with `--resume` keeps the session; two inputs written together while idle run strictly in order,
one turn each; `system/init` opens every turn; `result` carries a per-result cost in
`model_usage_since_last_result`. Not exercised: cancelling one queued input, and what the
transcript records for an input queued and then cancelled.

Not built yet: the bridge's request journal and `agentctl ledger` (12.1); the Python extension's
v1 events; `agentctl say`, `takeover` and `release-headless` (takeover is manual: stop, then
`claude --resume SESSION`); per-input cancel of a written input; the provenance header of section
10; the post-compaction records prompt of 9.5; `HeadlessDelivery` in the bridge (M3); Codex and Muse wrappers (after the
Claude cutover, per the owner's answer to question 5).

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

1. **Input: the queue agentctl already has, with a new owner.** An agent's `agent-queue`
   directory (`inbox/`, `inbox.v2/`, `inflight/`, `processed/`, `failed/`, `cancel/`,
   `cancelled/`) gets a new binding kind, `harness`. A queue bound that way has exactly one
   drainer: the headless wrapper. It hands each item to the harness over the harness's
   structured protocol instead of typing it into a composer. Everything else only enqueues and
   wakes the wrapper (section 5.1).
2. **Output: a typed JSONL event log** per agent (turns, assistant text, tool activity, input
   acknowledgements, compaction, status). Events **describe**; they never **act**. Effects such
   as chat replies and messages to other agents keep going through the existing idempotent
   commands (`agentctl chat reply`, `agentctl send`), so re-reading a log cannot repeat an effect.
3. **One wrapper per harness**, each a small executable that meets one contract (section 6).
   Claude Code comes first: a persistent `claude -p --input-format stream-json --output-format
   stream-json` process, which is **verified** to accept input in the middle of a turn and to
   acknowledge each input by the caller's own ID. Codex (`app-server`, or `exec resume`) and Muse
   (`muse serve`, or `muse exec --session-id`) follow.
4. **`agentctl tail NAME`**, a generic read-only follower of any headless agent's event log, so
   a headless agent can still have a Herdr tab. Closing the tab never touches the agent.
5. **The chat bridge** delivers to a headless coordinator by enqueueing, takes the queue's
   settled state (the model **started** the input) as proof of delivery, and gets replies only
   through `agentctl chat reply`. Screen capture is switched off for that target.

Milestones: M1 event log, ledgers and `tail`; M2 the Claude wrapper; M3 bridge integration; M4
a shadow day; M5 cutover, with the interactive coordinator kept as the rollback (section 13). An
optional **liaison** reader (section 12) is designed in from the start: it reads the same records
as the coordinator and has no write path into the coordinator.

## 1. Why headless

Problems the TUI coordinator has today, each recorded in this repository:

| Problem | Where recorded | Headless effect |
|---|---|---|
| One typist at a time; owner chat waited behind agent sends for up to 56 min | `#556 chat-delivery-latency` | one drainer per queue, writing to a pipe; no readiness wait |
| Submission proof reads the target and every peer pane (1-2.5 min per delivery on a loaded terminal server) | `#556` | the harness reports each input's lifecycle by ID (section 3.1) |
| Composer, shell-mode and scroll-pill screen dependencies | `#559 claude-shell-mode-prompt`, `#397` | none: no screen is read |
| Replies scraped between marker lines; a reply printed in thinking, or redrawn away, is lost | chat user guide, `--offer-reply-command` | replies only through `agentctl chat reply` (already exists) |
| Full-screen renderer folds tool output; readers see "scrollback jumps" | coordinator observation 2026-10-26 | `tail` renders the event log, not a screen |
| Channels unavailable with cloud-provider authentication; next-turn only | [channels note](../2026-10-26-claude-channels-bridge.md) | stream-json needs no channel feature and delivers mid-turn |

What is given up: the owner can no longer type into the coordinator's own TUI. Section 7.3 keeps
two ways to talk to a headless agent directly, and a takeover path back to the TUI.

## 2. What exists today (verified in this tree)

- **`agent-queue`** (`rs/agent-queue/src/lib.rs`, `queue.rs`): durable per-agent prompt queues,
  one identifier per phase, and the at-most-once barrier (`inflight/` plus `possibly_submitted`)
  before typing. A drainer holds the delivery lock for its whole drain and begins with
  `recover_inflight`, which quarantines every `inflight/` item as `possibly_submitted`. The one
  exception is an item stamped `paste_tracking` + `inflight_from` with no `paste_started_at`,
  which provably was never typed and goes back to its lane. The `inbox.v2/` lane adds
  tombstones and `not_after` (`#506 agent-queue`, inventory in
  [2026-10-25-agent-queue-lane-inventory.md](../2026-10-25-agent-queue-lane-inventory.md)).
  `target.json` binds a queue to one terminal (`pane` or `session`), and a drainer refuses a
  queue bound to a different target.
- **`agentctl send`** enqueues and then drains inline (`rs/agentctl/src/agent.rs`), so today
  every sender is also a drainer.
- **`agentctl inbox`**: coalesced, priority-ordered notices to a coordinator, delivered as one
  batch through the `print` or `agentcloud-notify` adapter.
- **The Python headless worker extension** (`py/agentctl/foreign/agent_runner.py`): a per-agent
  turn runner, recorded as `adapter: turn-runner`, `mode: headless`. It blocks on the agent's
  inbox and runs one `codex exec resume`, `agy --print` or `muse exec --session-id` subprocess
  per turn. It appends the JSONL events to `transcript.log`, publishes `last-message.txt`, and
  writes a `===TURN-DONE <seq> rc=<n> <iso>===` sentinel; a Herdr tab tails the transcript. It
  supports Codex, AGY and Muse, **not Claude Code**. `agentctl start --mode headless` exists
  for it, and profiles allow headless only for `codex`, `agy` and `muse`
  (`rs/agentctl/src/profiles.rs`). The Rust edition records such workers but "does not drive turn
  runners" (`rs/agentctl/src/subagents/turn_runner.rs`).
- **The chat bridge** separates coordinator delivery behind the `CoordinatorDelivery` trait
  (`rs/agentctl/src/chat_runtime.rs`): `message_state`, `submit`, `drain`, `drain_reporting`,
  `submit_once` and `withdraw` (the last two serve reply reminders), plus screen access. It keeps
  durable request records (one JSON file per admitted request, with phase, delivery and reply
  ordinals). `agentctl chat reply --bridge-state DIR --request KEY --reply-id ID --file F`
  stores a reply and wakes the service, with no screen involved.
- **agentcloud sessions** are already headless: `agentcloudctl notify` delivers mid-turn with an
  idempotency key, and `interrupt`, `wait`, `history` and `tail` exist. The headless contract
  below should be satisfiable by that adapter too.

The new work is narrow: the `harness` binding and its enqueue-only senders, a Claude driver in a
persistent process, a typed event log, `tail`, and a headless implementation of
`CoordinatorDelivery`.

## 3. Harness capabilities

### 3.1 Claude Code (verified, 2.1.310, through a cloud-provider gateway)

| Capability | Result | How observed (appendix) |
|---|---|---|
| `-p --output-format stream-json --verbose` | `system/init`, `assistant`, `user` (tool results), `result` with usage and `total_cost_usd` | E1 |
| `--session-id UUID` then `-p --resume UUID` | same session; recall across processes | E2 |
| Persistent process, `--input-format stream-json` | one process, many turns, a `result` per turn | E3, E5 |
| **Caller-supplied input ID** | the `uuid` we send comes back as `command_lifecycle` `queued` → `started` → `completed`, with `command_uuid` equal to it; an interrupted running input ends `cancelled` | E3, E5 |
| **Mid-turn input** | a message sent while a tool ran was `queued` at 7.06 s and `started` at 7.70 s, right after the tool result, inside the same turn; one `result` answered both | E3 |
| Replay acknowledgement | `--replay-user-messages` re-emits each input with `isReplay: true` | E3 |
| Receiver-side record | the session transcript holds a `user` entry whose `uuid` equals the caller's ID (inputs sent while idle, E5), or a `queued_command` attachment with that `source_uuid` and a `queue-operation` `absorbed_mid_turn` (mid-turn, E3) | E3, E5 transcripts |
| Interrupt | `control_request {subtype: interrupt}` → `control_response {still_queued: []}`; the running input ends `cancelled`; the turn ends `error_during_execution`; the next input runs normally | E5 |
| Compaction | `/compact` as an ordinary input: `status: compacting` → `compact_boundary {trigger, pre_tokens, post_tokens}`; recall survived | E5 |
| Permission denial | a deny rule yields a tool error and `result.permission_denials[]` | E5 |
| Turns the harness starts itself | a finished background command produced `task_notification` and a new turn with no input | E3 |
| Hooks | user and project hooks run in `-p` (the owner's own PreToolUse hook refused a foreground `sleep`) | E3 |
| Prompt cache | persistent process, turn 2: 77 140 tokens read from cache, 622 written. One per-process `--resume` (E2) wrote 30 363 again. A single observation each, both on the same model. | E2, E3 |

The `init` event also advertises capabilities, among them `msg_lifecycle_v1`,
`interrupt_cancel_queued_v1` and `interrupt_send_now_v1`. Only the lifecycle and a plain interrupt
were exercised. **To verify in M2:** `--input-format stream-json` combined with `--resume`;
cancelling one queued input; strict FIFO when several inputs queue at once (no run queued more
than one); and what the transcript records for an input that was queued and then cancelled before
it started.

Two cautions from the same runs. First, under `--permission-mode dontAsk --permission-prompts
none` with no allow rule on the command line, a Write and a Bash `touch` both ran (E4). The user,
project and managed settings were loaded and may have allowed them, and this host's launcher
sandbox may auto-approve sandboxed commands; the run does not separate the two. Only an explicit
deny rule refused the `touch` (E5). The permission policy must therefore be tested on the target
host with its real settings, and must include deny rules (section 9.4). Second, `total_cost_usd`
is cumulative for the session, as the public docs state for resumed sessions
([headless](https://code.claude.com/docs/en/headless): "the run reports the conversation's whole
total").

The public docs call streaming input "the **preferred** way" to use the Agent SDK. That is the
same protocol the SDK drives
([streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)). They also
recommend `--bare` for scripted calls ([headless](https://code.claude.com/docs/en/headless)). A
coordinator needs its `CLAUDE.md`, skills and hooks, so it should **not** run bare; the wrapper
records what it loads instead.

**Agent SDK or CLI?** The SDK is a library around the same stream-json child process. A Rust
wrapper that speaks stream-json directly avoids a Node or Python runtime and keeps the suite's
language. Keep the SDK as the reference for message shapes.

### 3.2 Codex (verified: `exec`; from schema and help: `app-server`, `queue`)

- `codex exec --json --skip-git-repo-check PROMPT` → `thread.started {thread_id}`,
  `turn.started`, `item.completed {agent_message}`, `turn.completed {usage}`. `codex exec --json
  resume THREAD PROMPT` kept the thread ID and recalled the first turn, with 23 552 of 31 162
  input tokens cached (E6).
- `codex app-server` is a JSON-RPC protocol. Its generated schema (`codex app-server
  generate-json-schema`) includes the client requests `thread/start`, `thread/resume`,
  `turn/start`, `turn/steer`, `turn/interrupt` and `thread/compact/start`, and the notifications
  `turn/started`, `turn/completed`, `item/*`, `thread/compacted`, `thread/queue/changed` and
  `thread/tokenUsage/updated`. `turn/steer` is the mid-turn input. It was **not** exercised live.
- `codex queue --thread T --message TEXT` "queue[s] a message for an existing session"
  (`codex queue --help`; not exercised).

### 3.3 Muse (verified: `exec`; from schema and help: `serve`, `session-message`)

- `muse exec --json --session-id UUID -- PROMPT`, run twice with the same UUID, recalled the first
  turn (E7). Its JSONL is an event-sourced record stream: `schema_version`, `id`, `stream {kind,
  id}`, `sequence`, `recorded_at`, `record_type`, `durability`, `causation_id`, `payload_type`
  (`runtime.command.accepted`, `turn.input.user`, `run.output.delta`, `run.usage`,
  `run.terminal.completed` with a `command_id`, `task.lifecycle.*`). This is a good model for our
  own envelope (section 5.2).
- `muse serve` hosts sessions over stdio using its own wire protocol. The schema it exports (`muse
  schema generate-json-schema`) includes `session/start`, `session/resume`, `turn/start`,
  `turn/steer`, `turn/interrupt`, `turn/unqueue` and `session/compact`. It was not exercised live.
- `muse session-message send --target SESSION` sends cross-session messages (`muse
  session-message --help`; not exercised).

### 3.4 Summary

| | persistent process | mid-turn input | per-input lifecycle by caller ID | interrupt | compact | resume |
|---|---|---|---|---|---|---|
| Claude Code stream-json | yes (verified) | yes (verified) | yes (verified) | yes (verified) | yes (verified) | `-p` verified; with stream-json input to verify |
| Codex app-server | yes (schema) | `turn/steer` (schema) | to verify | yes (schema) | yes (schema) | yes (schema) |
| Codex exec resume | per turn | no | turn-level only | kill | no | yes (verified) |
| Muse serve | yes (schema) | `turn/steer` (schema) | to verify | yes (schema) | yes (schema) | yes (schema) |
| Muse exec | per turn | no | `command_id` per run (verified) | kill | thresholds | yes (verified) |
| agentcloud | hosted | `notify` (documented) | idempotency key (documented) | `interrupt` | hosted | hosted |

## 4. Lessons from an earlier prototype

The owner built a private prototype stack in another orchestrator in July and August 2026. It
added a per-turn headless CLI backend (`claude -p --resume`, `codex exec resume --json`) and a
daemon-backed backend beside the TUI one. It was never landed, and that codebase has since
standardized on the TUI. A report on it is kept outside this repository, because it names
internal systems. These lessons carry over:

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
   `agentctl capabilities` must report harness-wrapper support per binary.
4. **A dead backend is `unreachable`, not `busy`,** and a still-starting agent's empty output is
   truthful, not an error.
5. **Say when a reader fell behind retention** instead of silently skipping (section 7.1).
6. **Failover between backends does not move a conversation.** Each backend owns its own session
   store. Rollback from headless to the TUI works only because both drive the same harness
   session (section 7.3).

## 5. Protocol

### 5.1 Inbound: queue items, one drainer

**The binding.** A headless agent's queue is bound with a new `target.json` kind:

```json
{"kind": "harness", "agent": "coordinator", "adapter": "harness-wrapper",
 "harness": "claude", "session": "<uuid>", "expected_cwd": "/path"}
```

Every drainer refuses a queue bound to a different target (`agent-queue` format). M2 must
**verify** that every binary still in use (pinned bridge, watcher, older `agentctl` and
`herdr-agent` builds, the Python edition) refuses an unknown `kind` rather than parsing it as a
pane, and must add a test for it. Until that holds on a host, the bridge and watcher there are
upgraded before any queue is bound `harness`, the same rule as for the lanes.

**One drainer.** For a `harness` queue only the wrapper moves items out of the pending lanes and
runs `recover_inflight`. `agentctl send`, the chat bridge's `submit`, `agent-teams` delivery and
reminders become **enqueue-only** for such a target. They enqueue under the existing rules, then
wake the wrapper through its control socket (section 6.1) and return the item's ID. A send that
wants to wait polls `agentctl status` or `message_state` for the item. No sender takes the
delivery lock for longer than a serialized enqueue.

**Fields.** Headless adds optional fields; readers ignore unknown fields, as the format requires.

| Field | Type | Meaning |
|---|---|---|
| `text` | string, required | the input, as today |
| `id` | queue identifier | as today. It becomes the harness input ID. Claude's stream-json `uuid` must be a UUID, so a non-UUID ID maps to a UUIDv5 of it, recorded as `harness_input_id`. |
| `from` | string | sender: an agent name, `chat:<request key>`, `inbox`, `owner`, `timer` |
| `priority` | `urgent` \| `normal` \| `low` | delivery order within the queue; absent is `normal` |
| `deliver` | `now` \| `turn-boundary` | `now` hands the item over mid-turn if the harness supports it; `turn-boundary` waits until the current turn ends |
| `reply_route` | object | for chat requests: `{request, reply_id}`, so the coordinator knows how to answer |

Items that use these fields go to `inbox.v2/`, so drainers that predate the lanes never see them
(the team-messaging design, section 12.4).

**Order.** `urgent` before `normal` before `low`, then `queued_at`, then name. The wrapper writes
items to the harness one at a time in that order. Whether the harness keeps that order when
several inputs queue at once is to be verified in M2. Until then, the wrapper writes the next item
only after the previous one is `started`.

**Default policy.** Owner chat requests are `urgent` + `now`, so the owner is answered between
tool calls, not after the turn. Agent reports (child-done notices, watcher notices, and inbox
batches through a new `agentctl inbox deliver --via queue` adapter) are `normal` +
`turn-boundary`, so a burst of worker notices cannot derail the coordinator mid-step. Timers are
`low`.

**Delivery, end to end.** The wrapper holds the delivery lock from the move into `inflight/`
until the item settles or a bounded ack deadline (default 30 s) passes, and never across a turn.
Because it is the only drainer, nothing else waits on that lock.

1. Move the item to `inflight/` with the existing barrier stamps (`possibly_submitted`,
   `inflight_at`, `paste_tracking: true`, `inflight_from`) plus `harness_input_id`, and fsync.
2. Stamp `paste_started_at`, fsync, then write the input line to the harness's stdin. Reusing the
   existing stamp keeps the existing recovery rule correct: an item without it was never written.
3. On `command_lifecycle queued` for that ID, stamp `harness_queued_at`. This means the harness
   has it, not that the model has seen it.
4. On `started`, the model has it in context. Stamp `started_at`, move the item to `processed/`
   (`delivery_state: processed`), and only then write the `input.started` event. **The queue
   stamp is authoritative**; the event is a view of it.
5. A `cancelled` before `started` means the harness dropped it unseen. The item returns to its lane
   at its original position (stamps stripped, as `repend_never_typed` does), unless it has a
   tombstone, in which case it goes to `cancelled/`. A `cancelled` after `started` (an
   interrupted turn) leaves the item `processed`, since the model saw it, and logs
   `input.cancelled` for readers.
6. If no `queued` arrives within the ack deadline, the item goes to `failed/` as
   `possibly_submitted`.

**Recovery** when a wrapper starts, before it drains: for each `inflight/` item that has
`paste_started_at`, the wrapper looks the ID up in the harness's own session record. For Claude,
that is a transcript `user` entry with `uuid` equal to `harness_input_id`, or a `queued_command`
attachment with that `source_uuid` followed by an absorbing `queue-operation`. If found, the item
moves to `processed/` with `recovered: transcript`. Otherwise it is left for the standard
`recover_inflight`, which quarantines it as `possibly_submitted`. Items without
`paste_started_at` return to their lane under the existing rule. Nothing is replayed on a guess.
What the transcript records for an input queued and then cancelled is to be verified in M2.
Until it is, a lone enqueue `queue-operation` with no absorbing entry is treated as not proven.

The receiver-side record is what turns "possibly submitted" into a definite answer. A harness
without one (per-turn `exec` modes) keeps the queue's existing at-most-once guarantee and nothing
stronger, and its contract (6.3) says so.

**Cancel and expiry.** A tombstone or an expired `not_after` found after the move to `inflight/`
sets the item aside unwritten, as today. Cancelling an item already written but not yet started
depends on per-input cancel, which Claude advertises (`interrupt_cancel_queued_v1`) but which was
not exercised. Until it is verified, the answer to such a cancel is "too late".

**Interrupt** is not a queue item. It is a control request on the wrapper's control socket, from
`agentctl interrupt NAME` (a proposed command).

### 5.2 Outbound: the event log

```text
<registry>/<agent>/events/<generation>.jsonl   0600, directory 0700, append-only
<registry>/<agent>/events/index.json           written only by the wrapper
```

Envelope, one object per line, each written with a single `write(2)`. The wrapper fsyncs after
every `input.*` event and at turn boundaries.

```json
{"v": 1, "seq": 4182, "id": "evt-...", "ts": 1793059038226, "agent": "coordinator",
 "generation": 7, "session": "<harness session id>", "turn": 311,
 "causation": "<queue item id or null>", "type": "tool.started",
 "data": {"tool": "Bash", "summary": "git fetch origin", "tool_use_id": "toolu_..."}}
```

- `seq` increases by one per event for the agent, across generations. A starting wrapper takes
  the larger of `index.json`'s `last_seq` and the last complete line of the newest generation
  file (a crash can leave `index.json` stale; a partial final line is truncated before
  appending). `seq` is therefore a stable address for links (section 12). `ts` is Unix
  milliseconds.
- `causation` links an event to the input that caused it, as Muse's `causation_id` does.

Event types (v1):

| Type | Data | Notes |
|---|---|---|
| `lifecycle.started` / `lifecycle.stopped` | wrapper and harness versions, argv digest, permission profile digest, exit cause | one pair per generation |
| `lifecycle.takeover` / `lifecycle.released` | the interactive tab and session | the log has no events between them; the harness transcript covers the gap (7.3) |
| `input.queued` | `queue_id`, `harness_input_id`, `from`, `priority`, `deliver` | the harness has it |
| `input.started` / `input.completed` / `input.cancelled` | `queue_id`, `mid_turn: bool` | from the harness lifecycle, after the queue stamp |
| `turn.started` / `turn.completed` | outcome (`completed`, `interrupted`, `error`), usage, cost delta, duration | `turn.started` also for turns the harness starts itself (`cause: background-task`, `wakeup`) |
| `assistant.text` | text, final per message (`partial: true` only when streaming deltas are enabled) | the coordinator's prose, verbatim |
| `tool.started` / `tool.completed` | tool, bounded summary (first 200 bytes of the command or path, with known secret patterns masked), `ok`, output byte count | never the output itself |
| `permission.denied` | tool, summary, rule | from the harness's denial list |
| `compact.started` / `compact.completed` | trigger, pre and post tokens | section 9.5 |
| `status` | `starting`, `idle`, `busy`, `unreachable`, `stopped` | derived; replaces screen inference |
| `error` | class, message | protocol or harness errors |

Thinking blocks are not logged (Claude returns them signed and empty in stream-json anyway). A
reader that finds a hole in `seq` shows a gap line; that is a rendering, not a logged event.

**Events describe, commands act.** The coordinator still replies to chat with `agentctl chat
reply` and messages agents with `agentctl send`, through its shell tool. Those commands are
already durable and idempotent per reply ordinal or message ID. The log records them as
`tool.started` / `tool.completed` like any other tool. The plan's `chat_reply` and
`send_to_agent` events are therefore **derived views** that `tail` builds from those tool events.
Nothing consumes them to cause an effect, so reading or replaying a log can never post a reply
twice. Section 15 asks whether a later version should add an effect channel (a wrapper-provided
MCP tool).

**Raw harness stream.** It is off by default. `--raw-log` (debugging only) keeps it at 0600 for
24 hours. It contains full tool output, so it never serves as a record and is never linked.

**Retention.** Records cite events by `seq`, so event logs are kept by age, not by generation:
90 days by default, subject to a 2 GiB cap, and the oldest generations are pruned first. The
wrapper is the only writer of `index.json` and does the pruning itself, at start. Ledger entries
that cite an event copy the cited text (an excerpt of up to 600 bytes) beside the link, so a
record stays readable after its event is pruned (section 12.1).

## 6. The wrapper

### 6.1 Contract

Each harness gets one executable, `agentctl-harness-<harness>` (first `agentctl-harness-claude`).
It is found the way chat plugins are found (manifest only, nothing executed during discovery),
and agentctl launches it. The agent record is `adapter: harness-wrapper`, `mode: headless`. That
keeps it distinct from `adapter: turn-runner` (the Python extension's per-turn workers, which the
Rust edition refuses to drive). `profiles.rs` gains `claude` as a valid headless harness for this
adapter only.

```text
agentctl-harness-claude run --registry DIR --agent NAME --generation N
    --cwd DIR --session UUID [--resume] --profile FILE [-- HARNESS_ARGS...]
agentctl-harness-claude capabilities        # prints the contract declaration (6.3) as JSON
agentctl-harness-claude conformance --fake  # runs the assertion table against a fake harness
```

While it runs, the wrapper:

- launches one harness child in its own process group (and the slot's cgroup when slotted),
  pinned by kernel process generation as agentctl already does for panes;
- is the queue's only drainer (5.1);
- writes the event log (5.2);
- serves a control socket, `<registry>/<agent>/control.sock` (0600, no network listener), with
  `wake`, `interrupt`, `cancel-queued ID` (answers "too late" until per-input cancel is verified),
  `status` and `stop`. agentctl's commands use it;
- on harness exit, records the cause. It restarts the harness with `--resume SESSION` only if the
  profile allows it, with backoff, and otherwise exits and lets systemd decide (section 9.1);
- on SIGTERM, stops taking new items, closes the harness's stdin, waits for the harness to exit
  (bounded), and writes `lifecycle.stopped`.

### 6.2 What stays out of the wrapper

The wrapper does not choose work or retry inputs on its own, and it does not render anything
(that is `tail`'s job). It holds no chat state (the bridge keeps it) and does not touch the
network beyond what the harness does.

### 6.3 Declared contract and conformance

`capabilities` prints the wrapper's declaration, which agentctl and the bridge read before
choosing a delivery policy:

```json
{"schema": "agentctl-harness-capabilities/v1", "harness": "claude", "harness_version": "2.1.310",
 "persistent": true, "mid_turn_input": true, "input_lifecycle": "by-caller-id",
 "receiver_record": "transcript-uuid", "interrupt": true, "cancel_queued": false,
 "compact": "command", "resume": "session-id", "self_started_turns": true,
 "send_boundary": "acceptance", "settle_on": "started", "streams_text": "per-message"}
```

The conformance table, adopted from the earlier prototype's lesson, checks behaviour against the
declaration. It runs with a fake harness (a scripted stream-json peer) and, opt-in, against the
real one. Example assertions:
- enqueue returns at acceptance within the declared deadline;
- a lifecycle event carries the caller's ID;
- a mid-turn input starts before the turn's `result`;
- an item is `processed` only after `started`;
- a crash at each step of 5.1 recovers to the documented phase;
- another drainer refuses a `harness`-bound queue;
- `status` never stays `busy` after the harness dies.

Every assertion has a negative test that breaks exactly one property.

### 6.4 Codex and Muse wrappers

- **Codex, M2b**: `app-server` over stdio, using `thread/resume`, `turn/start` and `turn/steer`
  for `deliver: now`. Fallback: the Python extension's `exec resume` loop (per turn, no mid-turn
  input, `input_lifecycle: turn-level`). The contract marks the difference, and the bridge then
  uses `turn-boundary` delivery.
- **Muse, M2c**: `muse serve`, using `session/resume`, `turn/start` and `turn/steer`. Fallback:
  the existing `muse exec --session-id` loop.

Both must pass the same conformance table, relaxed only where their declaration says so.

## 7. Viewing and talking to a headless agent

### 7.1 `agentctl tail NAME`

A read-only follower of one agent's event log. It takes no lock, and any number of copies can
run. (It is unrelated to the existing `agentctl read --output tail`, which reads a pane; the
userguide must say so.)

```text
agentctl tail NAME [--follow] [--since SEQ | --last N] [--json] [--types T,...]
                   [--width COLS] [--at SEQ]
```

- The default render is one line per event, with prose wrapped, tools on one line (`$ git fetch
  origin  ✓ 1.2 s`), inputs marked with their sender (`← owner (chat, mid-turn)`), and turn
  boundaries ruled with usage and cost.
- `--json` prints events verbatim, for scripts and the liaison (section 12).
- `--follow` uses inotify where available and falls back to a 250 ms poll. It follows across
  generations through `index.json`.
- A cursor older than retention, or a hole in `seq`, prints a gap line with the missing range.
- `--at SEQ` prints the event with that `seq` and its turn, which is the target of record links.

Starting a headless agent (M2: `agentctl start NAME --mode headless` with a profile whose adapter
is `harness-wrapper`) opens a Herdr tab running `agentctl tail NAME --follow`. That tab is a
view: Herdr's agent status for it is unknown, and agentctl reports status from `status` events
instead. `tail` is generic, so it works for any harness that writes the log, including the Python
extension's workers once their runner writes v1 events beside `transcript.log`.

### 7.2 Status for watchers

`agentctl status NAME` and `agentctl watch-agents` read the last `status` event and the queue
instead of a screen:
- `idle` after `turn.completed` with no item pending;
- `busy` during a turn;
- **never `blocked`**, because under `--permission-prompts none` nothing can prompt, and a refused
  tool is logged as `permission.denied` instead;
- `unreachable` when the wrapper's control socket does not answer within 10 s while its process is
  alive;
- `stopped` when the wrapper is dead.

The watcher's "idle without DONE:/BLOCKED:" rule reads the last `assistant.text` of the turn.

### 7.3 Talking to it directly, and taking over

- `agentctl send NAME TEXT` enqueues and wakes the wrapper (enqueue-only for a `harness` queue,
  5.1). Its exit status says that the item was queued, and `--wait` waits until it settles.
- `agentctl say NAME` (proposed, M2) is an interactive line editor in a split of the `tail` tab.
  Each line becomes an `urgent` + `now` queue item from `owner`. This replaces typing into the
  TUI.
- **Takeover** (proposed, `agentctl takeover NAME`): it stops the wrapper cleanly at a turn
  boundary, writes `lifecycle.takeover`, rebinds the queue to the new pane, and starts the
  interactive TUI in a tab with `claude --resume SESSION`, the same conversation. The agent then
  is an ordinary interactive agentctl agent. `agentctl release-headless NAME` (proposed) stops the
  TUI, rebinds the queue to `harness`, writes `lifecycle.released`, and restarts the wrapper with
  `--resume`. While taken over, the event log has no events; `tail` shows the gap as a takeover,
  and the harness transcript is the record. This is also the rollback for M5.

## 8. Chat bridge changes (M3)

A new `CoordinatorDelivery` implementation, `HeadlessDelivery`, is selected when the target's
queue is bound `harness`:

| Trait method | Today (pane) | Headless |
|---|---|---|
| `submit` | enqueue, lifecycle lock, readiness wait, type, verify on screen | enqueue an `urgent` + `now` item with `reply_route`, wake the wrapper |
| `drain`, `drain_reporting` | type the queue | no-op (the wrapper drains); reports the queue's state |
| `message_state` | queue phases plus screen evidence | queue phases and stamps only: `processed` means the model started the input |
| `submit_once`, `withdraw` (reply reminders) | queue behind no pending prompt; withdraw if unsent | the same, against the pending lanes; a reminder is a `normal` item naming the unanswered request |
| reply capture | screen and transcript, alias gate | none; replies only through `agentctl chat reply` |
| receipt reaction | on verified submission | when the item is `processed` |

A request whose item lands in `failed/` as `possibly_submitted` becomes `delivery_uncertain` for
an operator, exactly as a pane delivery does today.

The latency that remains is the time until the coordinator's current tool call returns. For a
short tool that is under a second: in E3, 0.64 s passed from send to `started`. During a long
foreground command, it is the whole run. A headless coordinator should therefore run long
commands (gates of 35-40 min) in the background. Claude Code's background commands make that
easy, and the owner's own hook already pushes the agent that way (E3).

Channels remain an option for an interactive coordinator where Anthropic authentication is
available. Headless does not need them.

## 9. Operations

### 9.1 Process supervision

- One systemd user unit per headless agent, `agentctl-headless-<agent>.service`. It runs
  `agentctl run-headless NAME` (proposed), which execs the wrapper with the recorded launch spec:
  `Restart=on-failure`, `RestartSec=5`, `StartLimitIntervalSec=600`, `StartLimitBurst=5`,
  `KillMode=mixed`, `TimeoutStopSec=120`. `agentctl install-units` (which today writes the watch
  service and a bridge drop-in) learns to write it. The unit, not `restore --boot`, starts a
  headless agent. `restore` skips agents whose record names this unit, so nothing starts twice.
- The wrapper never runs two harness processes for one session. Startup takes an exclusive lock
  on `<registry>/<agent>/headless.lock` and refuses if a previous generation's harness, pinned by
  PID and start time, is still alive.

### 9.2 Restart and resume

On start the wrapper runs the persistent process with `--resume SESSION` when the record has a
session, and with `--session-id NEW` otherwise. That combination is to be verified in M2; the
fallback is a fresh process per restart with `-p --resume`, which E2 verified. It then runs
recovery (5.1) before draining. A resumed Claude session leaves an interrupted turn as it was
unless `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` ([headless](https://code.claude.com/docs/en/headless),
"When you resume the session"). The wrapper does not set it. Instead it enqueues one `normal`
item, "you were restarted at T; last event seq S; re-read your records", so the coordinator
re-reads its records.

### 9.3 Crash recovery

| Crash point | Queue state | Recovery |
|---|---|---|
| wrapper dies before writing an item | `inflight/`, tracked, no `paste_started_at` | back to its lane (existing rule) |
| wrapper dies after writing, before `started` | `paste_started_at`, maybe `harness_queued_at` | transcript lookup: `processed`, or `failed` `possibly_submitted` |
| wrapper dies after `started`, before the rename | `started_at` stamped | `processed` (the stamp proves it) |
| harness dies mid-turn | items settled as above | restart with `--resume`; restart notice queued; `status` `unreachable` until then |
| event log write fails (disk full) | the queue keeps every unsent item | the wrapper stops taking items and exits nonzero; systemd retries after space returns |

### 9.4 Non-interactive permissions

- The exact flags: `--permission-mode dontAsk --permission-prompts none`, plus `--settings
  <profile>` naming the coordinator's rules. Under them anything not allowed is denied and logged
  as `permission.denied`; nothing can wait for a human.
- The profile file has allow rules for the coordinator's tools (agentctl, wrkslots, herdr-run,
  git read commands, the gate). It also has **deny rules** for what must never run, such as force
  pushes, edits to live service units, and `rm -rf` outside slots. E4 showed that commands with no
  allow rule on the command line can still run under the host's settings or sandbox, so the deny
  list is the guard that holds.
- The profile is recorded by digest in `lifecycle.started`. A change requires the owner's
  approval, like the existing `--trust-workspace` decision.

### 9.5 Long sessions and context

- Auto-compaction stays on. `compact.*` events record every compaction with token counts, so a
  reader can tell when the model's memory was summarized.
- At `compact.completed`, the wrapper enqueues a `normal` item pointing at the records the
  coordinator should re-read (status ledger, open requests, active slots), in the role `gt prime`
  plays in Gas Town. Records, not memory, carry state across compaction; that is also the
  liaison's premise.
- `/compact` can be issued deliberately (E5) by `agentctl compact NAME` (proposed) at a quiet
  moment.

### 9.6 Cost and usage

`turn.completed` carries usage and the cost delta. `agentctl status` shows tokens and cost per
day. The persistent process keeps the prompt cache warm between turns (3.1).

## 10. Security

- **Authority is the filesystem**, as today: whoever can write the agent's queue directory (0700,
  owner-only; agentctl refuses a non-private queue) can instruct the agent. Headless adds no
  network listener. The control socket is 0600 in the registry, and the harness talks only over
  stdio.
- **No secrets in records.** Tool summaries are bounded, have known secret patterns masked, and
  never include environment values or tool output. The raw stream is off by default (5.2).
  Profiles name environment variables, never their values, as `agentctl status` already does.
- **Untrusted text is labelled.** Every item carries `from`, and the wrapper prefixes chat and
  agent-sent text with a one-line provenance header, using the same forgery-proof rendering as
  the inbox, so a message cannot pose as the owner or as a system notice.
- **Permissions** as in 9.4. The coordinator profile is reviewed like code.
- **Credentials.** The harness keeps its own authentication. The wrapper never reads or forwards
  it, and `--bare` (which changes the authentication sources) is not used.

## 11. What the owner sees

| Before | After |
|---|---|
| the coordinator's TUI in the "coord" tab | `agentctl tail coordinator --follow` in the same tab, plus `agentctl say` for typing |
| replies in chat via screen capture or `chat reply` | replies via `chat reply` only |
| folded tool output, scroll jumps | one line per tool, nothing folded or redrawn |
| `claude agents` shows it busy or idle | `agentctl status coordinator` shows it from events |

## 12. Optional liaison

The owner's concern is a "telephone game" between a liaison that stays responsive to the owner
and a coordinator that does the work. Each hop of paraphrase loses or distorts something. The
design therefore makes the liaison a **second reader of the same primary records**. It has no
path for passing anything to the coordinator, so it cannot relay a summary in either direction.

### 12.1 The primary records

Every record a liaison may cite is append-only or versioned, has a stable address, and is written
by the party that owns the fact:

| Record | Written by | Address | Exists today? |
|---|---|---|---|
| Coordinator event log (5.2) | the wrapper | `coordinator events#<seq>`; `agentctl tail coordinator --at SEQ` | new in M1 |
| Request ledger | **the chat bridge only** | request key; `agentctl chat thread`, `chat history`, `chat sent` | yes, as per-request JSON records. M1 adds an append-only `requests.jsonl` journal: each admitted owner message verbatim, each reply with who sent it, and each request's state. |
| Status ledger | the coordinator, through `agentctl ledger` (proposed) | `ledger#<seq>`; one entry per change (lane opened, slot, issue, state, open question) | informally (a hand-edited JSON file); M1 makes it an append-only journal with a current-state view |
| Handoffs | workers, through wrkslots | slot and generation; `wrkslots read-handoff` | yes |
| Chat history | the provider, mirrored by the bridge | provider permalink plus request key | yes |
| Issues | anyone, through `mb` | `#<number> <slug>` | yes |

A link is the address plus the command that resolves it, for example `coordinator events#4182
(agentctl tail coordinator --at 4182)`. A ledger entry that cites an event copies the cited
excerpt beside the link, so the citation survives the event's retention (5.2). The coordinator's
own replies should carry the same links.

**Owner decisions are already verbatim.** The bridge admits every owner message into the request
ledger with its exact text and permalink, and delivers that same text to the coordinator. Nobody
needs to re-enter a decision, and no one paraphrases it. A decision is found by reading the
owner's own message.

### 12.2 Ground rules (owner, 2026-10-26)

1. **Answers only from records, with links.** Every factual sentence in a liaison reply cites at
   least one record address, with the excerpt it relies on.
2. **Owner decisions are written verbatim to the request ledger.** The bridge does this on
   admission (12.1); the liaison neither writes nor restates decisions.
3. **Substantive answers come from the coordinator.** That covers plans, judgments, priorities,
   and anything not already recorded.
4. **Everything else gets "the coordinator has this" plus a link** to the request's ledger entry.
5. **The coordinator's own words are quoted, never paraphrased.** When a record the coordinator
   wrote (an `assistant.text` event, a status-ledger entry) is the answer, the liaison quotes it
   with its address and time: "the coordinator wrote at 14:02 (events#4182): ...". Quoting a
   primary record is not relaying a summary. Rewording it would be.

### 12.3 How the rules are enforced, not just requested

- **Allow-listed by construction.** The liaison runs with `--tools` limited to file reading over
  the record paths, plus one small MCP server (proposed) that exposes `records.read` (event log,
  ledgers, handoffs, chat thread) and `chat.reply`. It has no shell. The rules therefore do not
  depend on deny rules, which section 9.4 shows are the only reliable guard for a shell-capable
  agent.
- **No write path to the coordinator.** The liaison cannot enqueue to the coordinator and cannot
  write a ledger the coordinator reads as input. Its only write is its own chat reply, which the
  bridge records in the request ledger as `sent_by: liaison`. That needs a per-sender label in
  the bridge; today a bridge has one agent label.
- **The bridge routes; the liaison does not.** Every owner message is delivered to the
  coordinator's queue (5.1), exactly as without a liaison. The liaison receives a copy at the
  same time.
- **Double answers are expected, and made cheap.** Both receive the message at once, so the
  liaison may answer a status question while the coordinator is still on its current tool call.
  The coordinator's request prompt says that a liaison serves this channel. Before answering, the
  coordinator runs `agentctl chat sent --request KEY`, and it adds a reply only when the question
  needs judgment or the liaison's answer was the "coordinator has this" form. A liaison reply
  reaches the coordinator only that way: as the bridge's record of what was sent, read on demand,
  and never pushed into its queue.
- **Lint at the boundary.** `chat.reply` refuses a liaison reply in which a sentence has no
  record address and excerpt, unless the whole reply is the fixed "the coordinator has this:
  <link>" form. The lint proves a citation is present, not that it supports the sentence. The
  excerpt beside each link lets the owner check that at a glance.
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
need them. Trial a liaison only if the M4 shadow day shows owner questions waiting.

## 13. Migration plan

| Milestone | Deliverable | Exit criterion |
|---|---|---|
| **M0** (this note) | design, a report on the earlier prototype (outside the repository), issue `#565` | note landed; owner answers to section 15 |
| **M1** event log, ledgers, `tail` | v1 event schema and a writer library; `agentctl tail`; the bridge's append-only request journal; `agentctl ledger`; the Python headless extension writes v1 events beside `transcript.log` | `tail --follow` renders a live Python-extension worker; gap rendering tested; both ledgers written |
| **M2** Claude wrapper | the `harness` binding and enqueue-only senders; `agentctl-harness-claude` with recovery, control socket, `capabilities`, and a conformance table on a fake harness; `start` with the `harness-wrapper` adapter, `say`, `interrupt`, `compact`, `takeover`, `release-headless`; the M2 verification list in 3.1 | conformance green; every binary in use refuses a `harness` queue; a headless reviewer agent runs a real task; crash injection at each point of 9.3 |
| **M2b / M2c** Codex and Muse | `app-server` and `serve` wrappers, or the per-turn fallback | the same conformance table, relaxed per declaration |
| **M3** bridge integration | `HeadlessDelivery`; `inbox deliver --via queue`; reminders as queue items; status from events in `watch-agents` | a second, non-owner coordinator answers chat end to end headless; delivery latency measured (admission to `started`) |
| **M4** shadow day | a headless coordinator beside the interactive one, receiving copies of owner messages, with its replies logged and not posted | a written comparison; no lost or duplicated request across the day; permission denials reviewed |
| **M5** cutover | the bridge targets the headless coordinator; the TUI tab becomes `tail` plus `say`; the rollback (`takeover`) is documented and rehearsed | one week without needing takeover, or a recorded reason for each takeover |

Each milestone lands through the normal gate. Live services change only at M3 (a second
coordinator) and M5, each with the owner's approval and a recorded rollback.

## 14. Risks

| Risk | Mitigation |
|---|---|
| stream-json is a moving interface (2.1.310 today; `command_lifecycle` may change) | the wrapper pins a tested harness version range; `capabilities` reports the version; conformance against the real harness in M2 |
| an older binary drains a `harness` queue as if it were a pane | M2 verifies that unknown binding kinds are refused; upgrade drainers first; conformance test |
| a long foreground tool call delays mid-turn input | background long commands; `interrupt` for urgent cases; measure in M4 |
| harness-started turns (background tasks, wakeups) interleave with queued input | events carry `cause`; queue order is unaffected; `tail` shows it |
| host settings or sandbox approve more than the profile allows | deny rules; profile reviewed; denial log watched in M4 |
| the owner loses the "glance at the TUI" view | `tail` shows the same facts unfolded; `takeover` restores the TUI on demand |
| silent context loss at compaction | `compact.*` events; records-first recovery prompt (9.5) |
| two coordinators answering one request during the shadow day | replies suppressed in M4; the request ledger records who sent each reply |
| links rot as logs are pruned | age-based retention; excerpts copied into the citing record |

## 15. Questions for the owner

1. **Mid-turn policy.** Should owner chat always be `now` (answered between tool calls), or only
   when a message is marked urgent, with ordinary chat at the turn boundary?
2. **Effects through commands only?** v1 keeps chat replies and agent messages as shell commands
   (idempotent today). Do you want a later wrapper-provided MCP tool for replies instead (a
   cleaner tool call in the log, Claude-specific)?
3. **Takeover.** Is `takeover` (stop headless, open the TUI on the same session) the right escape
   hatch? A TUI attached read-only to a live headless session does not exist in Claude Code as far
   as checked.
4. **Liaison.** Build the records in M1 regardless (recommended), and decide on a liaison after
   M4? Which model would a liaison run (cheaper is the point)?
5. **Codex and Muse coordinators.** Should M2b and M2c come before M3, so the bridge supports all
   three from the start, or after the Claude cutover?
6. **Permission profile.** Who signs off the coordinator's allow and deny lists, and should an
   example live in this repository with host-specific parts kept private?
7. **Shadow day scope.** Is a day with suppressed replies enough, or should the headless
   coordinator first answer a second, low-stakes channel for real?

## 16. Related work

### 16.1 Gas Town (read at commit `de2ce8efa7aa`, 2026-10-25)

Sources: `docs/overview.md`, `docs/glossary.md`, `internal/templates/roles/mayor.md.tmpl`,
`docs/design/mail-protocol.md`, `docs/design/escalation.md`,
`docs/agent-provider-integration.md` and `docs/design/witness-at-team-lead.md` in
[github.com/steveyegge/gastown](https://github.com/steveyegge/gastown).

- **Roles.**
  - Infrastructure roles: the Mayor ("global coordinator", singleton); the Deacon ("background
    supervisor daemon", singleton); a Witness per project ("polecat lifecycle manager"); and a
    Refinery per project ("merge queue processor").
  - Worker roles: Polecats (persistent identity, ephemeral sessions, managed by the Witness);
    Crew (long-lived, managed by the user); and Dogs, the Deacon's infrastructure helpers. One
    Dog, Boot, checks the Deacon every 5 minutes.

  Sources: `overview.md`, `glossary.md`.
- **Who faces the owner.** The Mayor does. The glossary says it initiates convoys, coordinates
  work distribution, and notifies users of important events. Its role prompt says it "is a
  **coordinator**, not a solo coder" whose default response to a user request is `bd create` then
  `gt sling`. It lists "Work dispatch", "Escalation handling" and "Strategic decisions" as the
  Mayor's responsibilities, and "routine nudging (Witness handles that)" as **not** its job
  (`mayor.md.tmpl`). The human is the Overseer, the last tier of escalation: agent → Deacon →
  Mayor → Overseer (`escalation.md`).
- **The owner's question.** No Gas Town role separates owner-facing responsiveness from
  dispatch: the Mayor does both. What Gas Town separates is **monitoring**. The Deacon, the
  Witnesses and Boot patrol the workers, so the Mayor is not interrupted by routine health
  checks. Its "Propulsion Principle" (execute hooked work without waiting for confirmation)
  addresses the opposite failure: a coordinator that waits for the owner and stalls the town.
- **Messaging** (`mail-protocol.md`).
  - `gt mail` creates a persistent bead in the Dolt-backed tracker, so mail survives the
    recipient's session death.
  - `gt nudge` sends text "directly to an agent's tmux session", where it appears as a system
    reminder; it is lost if the session is dead.
  - The rule: "Default to `gt nudge`. Only use `gt mail send` when the message MUST survive the
    recipient's session death." Each role has a mail budget.
  - Beads also implement groups, claim-once queues and pub/sub channels.
- **Work tracking.** Beads (issues) and convoys (batches of beads that "land" when their issues
  close) are the ledger. The Mayor's prompt calls completions "a permanent ledger of demonstrated
  capability" (`overview.md`, `mayor.md.tmpl`).
- **Transport** (`agent-provider-integration.md`, `witness-at-team-lead.md`).
  - Tier 0 integration is tmux `send-keys` and `capture-pane`, which the docs call
    "timing-sensitive and has no delivery confirmation".
  - Non-interactive `exec` modes are used for formulas and Dogs.
  - The forward-looking provider contract adds `SendMessage()` and `GetStatus()` to replace
    `send-keys` and screen scraping.
  - A design, not implemented, would move Witness-managed polecats onto Claude Code agent teams.
- **Taken here:** records over memory after compaction (`gt prime` ↔ the post-compaction
  prompt in 9.5); patrol separated from the coordinator (already true here: `watch-agents`);
  durable versus ephemeral messages (queue items versus interrupts). **Not taken:** tmux as the
  coordinator's transport.

### 16.2 Protocols and SDKs

- **A2A** (read at [a2aproject/A2A](https://github.com/a2aproject/A2A) commit `98279d623163`,
  `docs/specification.md` sections 3.4-3.5, `specification/a2a.proto`).
  - Tasks have the states `submitted`, `working`, `input-required`, `completed`, `failed`,
    `canceled`, `rejected` and `auth-required`.
  - `contextId` groups tasks and messages into one conversation.
  - Events must be delivered in the order they were generated. When several streams serve one
    task, each receives the same events in the same order.
  - Taken: ordered event streams with several readers (5.2, 12.3).
  - A2A is a network protocol between agents. This design stays local and file-based, though the
    event schema could later be bridged to A2A task events.
- **MCP**: the stdio tool protocol that harnesses already speak. It matters here as the possible
  effect channel of question 15.2, for the liaison's `records.read` and `chat.reply` server, and
  as the basis of Claude Code channels (see the
  [channels note](../2026-10-26-claude-channels-bridge.md)).
- **Claude Agent SDK streaming input**: a persistent session that queues messages, takes
  interrupts and surfaces permission requests, which the docs call the preferred mode
  ([docs](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)). The wrapper
  speaks the same stream directly. SDK subagents run inside one harness process and report to
  their parent natively. They suit short delegated tasks; persistent, separately visible workers
  stay with agentctl.
- **Codex app-server**: already compared in
  [common/docs/agentctl/RELATED_WORK.md](../../../common/docs/agentctl/RELATED_WORK.md).
  Section 3.2 adds the verified `exec resume` behaviour and the schema's `turn/steer`.

### 16.3 Orchestrators

- **OpenHands** (read at [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands) commit
  `dbe578d5dbac`, `README.md`). The frontend repository now delegates to an Agent Server, "a REST
  API for running multiple agents on a single machine", implemented in its software-agent SDK
  together with "conversations, workspaces, events". A separate automation service "decides when
  work runs", and the agent server "decides what runs". Taken: the split between deciding when to
  run (here, queue policy and the bridge) and running (here, the wrapper).
- **CLI Agent Orchestrator, Happy, Claude Squad**: compared in
  [common/docs/agentctl/RELATED_WORK.md](../../../common/docs/agentctl/RELATED_WORK.md). CLI
  Agent Orchestrator marks a message delivered before the paste, the same choice as agentctl's
  `inflight/` barrier. Headless adds a receiver-side record that turns "possibly submitted" into
  a definite answer.
- **Cross-host messaging systems** (Matrix, NATS, AutoGen, MCP Agent Mail): compared in the
  [team-messaging related work](../2026-10-24-agentctl-team-messaging/RELATED_WORK.md). The
  headless queue is the last hop that those designs end at, without the terminal composer.

## Appendix: experiments (2026-10-26, this host)

All runs used a scratch directory outside any Git working tree and cheap prompts. Commands are
abbreviated. The driver scripts and logs are kept with the report outside the repository.

- **E1** `claude -p --model haiku --output-format stream-json --verbose --session-id U "..."`:
  23 `hook_started` and 23 `hook_response` events, then `init` (`apiKeySource: apiKeyHelper`),
  `assistant` and `result`. The session ID matched U.
- **E2** the same with `--model sonnet`, then `-p --resume U "What word ..."`: the answer was
  `PLUM-42`. Usage: turn 1 wrote 24 415 cache tokens; the resumed process wrote 30 363 and read
  11 915.
- **E3** a persistent process with `--model sonnet --input-format stream-json
  --replay-user-messages --permission-mode dontAsk --allowedTools 'Bash(sleep *)'`.
  - Message 1 asked for `sleep 25; echo done-sleeping`. The owner's PreToolUse hook refused the
    foreground form, and the model moved it to the background.
  - Message 2 was sent at 7.06 s, `queued` at 7.06 s and `started` at 7.70 s, and was answered in
    the same turn (`result` at 8.86 s).
  - The background task's completion started a turn by itself (`result` at 35.23 s).
  - The transcript recorded message 2 as a `queued_command` attachment with its `source_uuid`,
    then a `queue-operation remove reason=absorbed_mid_turn`.
  - Turn 2 usage: 622 cache tokens written, 77 140 read.
- **E4** `--permission-mode dontAsk --permission-prompts none` with no allow rule on the command
  line: a Write and a Bash `touch` both ran. The host's user, project and managed settings were
  loaded.
- **E5** a persistent process with `--disallowedTools 'Bash(touch *)'`.
  - The `touch` was denied and listed in `permission_denials`.
  - `/compact` gave `compact_boundary {trigger: manual, pre_tokens: 37807, post_tokens: 12768}`,
    and recall afterwards was correct.
  - The inputs sent while idle appear in the transcript as `user` entries whose `uuid` is the
    caller's ID.
  - A long essay request, then a `control_request interrupt` about 4 s after it started, produced
    `control_response {still_queued: []}`, `[Request interrupted by user]`, `cancelled` for the
    running input, and `result error_during_execution`. The next input was answered normally.
- **E6** `codex exec --json --skip-git-repo-check "..."`, then `codex exec --json resume T
  "..."`: the thread ID was stable and the answer was `FIG-17`. Usage: 15 541 input tokens (8 448
  cached), then 31 162 (23 552 cached).
- **E7** `muse exec --json --reasoning-effort low --session-id U -- "..."`, twice: the answer was
  `KIWI-9`, and the run ended with `run.terminal.completed`, whose payload carries `command_id`.

# agent-queue: inventory of queue writers and readers for the new lanes

Dated record, 2026-10-25, for `#506 agent-queue` phase P2 (`#496 agentctl-team-messaging`,
design section 12.4). It lists every program in this repository that writes into, moves items
through, or decides anything from an agentctl prompt queue, and says how each one treats the
three lane directories P2 adds (`inbox.v2/`, `cancel/`, `cancelled/`) and the `not_after` field.
Local `agentctl send --ttl` and `agentctl cancel` are enabled only because every entry below is
covered.

The format is documented in the `agent-queue` crate (`rs/agent-queue/src/lib.rs`).

## Rust: the agent-queue crate

| Function | Before P2 | After P2 |
|---|---|---|
| `queue::enqueue` (duplicate check) | four phases | every item directory, including `inbox.v2/` and `cancelled/`; refuses `not_after` on an `inbox/` item |
| `queue::enqueue_into(Lane::InboxV2, ...)` | none | lane created on first use; `not_after` must be integer milliseconds |
| `queue::message_state` | four phases | adds `inbox.v2/` (pending) and `cancelled/` (new state `cancelled`) |
| `queue::pending_identifiers` | `inbox/` | `inbox/` and `inbox.v2/` |
| `queue::withdraw` | `inbox/` | `inbox/` and `inbox.v2/` |
| `queue::pending_in_queue_order` | n/a | merges both pending directories by `queued_at`, then name |
| `queue::cancel`, `set_aside_reason`, `set_aside` | n/a | tombstones and the pre-paste check |
| `queue::validate_existing_queue` | four phases | also validates any lane directory present |
| `queue::prepare` | creates the four phases | unchanged: lanes are created only by what needs them |

## Rust: agentctl

Every agentctl path that writes a queue goes through `agent.rs`, which calls the crate:

| Path | Writes | Lane handling |
|---|---|---|
| `agentctl send` (`agent::send*`, `ManagedAgents::send_identified*`) | enqueue, then drain | `--ttl` puts the prompt in `inbox.v2/` with `not_after`; a send whose prompt is set aside fails with the reason |
| `agentctl drain`, every managed drain | moves items | takes both pending directories in queue order; checks tombstone and expiry before readiness and again after the move to `inflight/`; `--deadline`, `--lock-timeout`, `--max-items` |
| `agentctl cancel` | tombstone only | no lock, so it never waits behind a drain |
| `ManagedAgents::send_once_with_runtime` (chat reply reminders) | enqueue, drain, withdraw | refuses to queue behind any pending prompt in either directory; a reminder found set aside counts as withdrawn |
| chat bridge (`chat_runtime.rs`, `chat_service.rs`) | requests, fence feedback, reminders through the managed send | every `message_state` match handles `cancelled`: a request set aside untyped becomes `delivery_uncertain` for an operator and is never queued again under its ID |
| `agentctl watch-agents` notifier (`agent_watch.rs`) | batches through the managed send | a cancelled batch is not sent again; `has_pending` reads status rows, whose `pending` list includes `inbox.v2/` |
| `agentctl watch`, `restore` notices (`restore.rs`) | through the managed send | no special case: they never set `not_after` |
| `agentctl status` (`agent::status*`) | none | `pending` lists both pending directories |
| `revive`, `stop` | move the whole queue directory to the archive | lanes travel with it |
| `herdr-agent` (`legacy_cli.rs`) | same `agent.rs` functions | same handling |

## Rust: agent-teams

| Path | Writes | Lane handling |
|---|---|---|
| `agentctl-queue` deliverer (`rs/agent-teams/src/deliver.rs`) | enqueue only, unserialized (the ledger claim makes the ID single-writer) | team items go to `inbox.v2/` through `enqueue_into(Lane::InboxV2, ...)` with `not_after` in milliseconds; an item found `cancelled` counts as handed over, so recovery never hands it over again |

## Python edition

The Python edition gets no port of the lanes (owner, 2026-10-25). It gets a guard instead,
`agentctl.agent.queue_lane_refusal`:

| Path | Guard |
|---|---|
| `agent._enqueue` (every send, chat request, chat feedback, legacy CLI) | refuses a queue with items in `inbox.v2/` or `cancel/`, or an `inbox/` item carrying `not_after` (only regular files are read, so a FIFO planted under a `.json` name cannot block it); its duplicate check also covers `inbox.v2/` and `cancelled/`, so a set-aside prompt is never queued again |
| `agent._drain` (every drain, every send's drain) | same refusal; `cancelled/` alone does not block it |
| chat runtime re-enqueue of a `received` request (`chat_runtime.py`) | counts an item in `inbox.v2/` or `cancelled/` as queued, so it is never prompted again; the drain then refuses |
| chat bridge re-enqueue (`chat.py`) | goes through `agent.enqueue`, which refuses |

`python -m pytest py/tests/test_herdr_agent.py -k rust_only_lanes` checks the refusal for each
lane, and the cross-edition differential (`cross/agentctl_differential.py`, `_queue_lanes`)
checks that the Python edition refuses to drain or send into a queue the Rust edition gave a
second inbox and a tombstone, typing nothing, and that the Rust edition then types the
expiring prompt once and sets the cancelled one aside.

## Outside this repository

- Pinned older agentctl binaries (a chat bridge or watcher started from a fixed build) never list
  `inbox.v2/` and never read tombstones: they leave second-inbox prompts alone and may type an
  `inbox/` prompt that has a tombstone. `agentctl cancel` therefore sets a waiting prompt aside at
  once whenever no drain holds the queue (outcome `set_aside`, safe against every drainer); a
  `pending` outcome is honoured only by current drainers. An older chat runtime's
  `message_state` does not see `cancelled/` and may re-queue a cancelled chat request: upgrade a
  queue's drainers (bridge, watcher) before using `cancel`, `send --ttl` or agent-teams on it.
- The per-host helper of a consuming project (design section 2.3) checks the four phases by name.
  It must learn `inbox.v2/` and `cancelled/` before it relies on `message_state`, or become the
  thin wrapper of design section 15.1.
- Topics and `--supersedes` (design 12.1) need a ledger and are left to agent-teams (P3).

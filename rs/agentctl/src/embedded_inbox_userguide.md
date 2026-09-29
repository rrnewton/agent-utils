# agentctl inbox reference

## The problem

A coordinator agent dispatches work to several persistent workers. Native subagents of a harness
report back automatically when they finish; persistent workers in their own terminals or hosted
sessions do not. Without help, the coordinator either polls every worker's output on a timer or
lets workers type into its session directly, which is unauthenticated, lands in the middle of its
work, and turns five simultaneous reports into five interruptions.

`agentctl inbox` is the queue between them. Notices are posted by workers, or by a watcher acting
for them. They are coalesced, and then delivered to the coordinator as one batch, in priority order.

## Dependencies

None for `post`, `list`, `render` and `deliver --via print`: they only read and write files under
the registry. `deliver --via agentcloud-notify` runs `agentcloudctl` (override with the global
`--agentcloudctl-bin`).

## Layout

Everything lives under `<registry>/.inbox/<coordinator>/` (the registry is the global
`--registry`, default `.agentctl`):

| Path | Meaning |
|---|---|
| `live/<created-ms>-<id>.json` | one queued notice |
| `claimed/<batch>.json` | a batch handed to an adapter whose outcome is not yet known |
| `delivered/<batch>.json` | delivered batches; the newest 200 are kept |
| `.lock` | held for every change to the queue |
| `.deliver.lock` | held for a whole delivery; a second concurrent delivery exits 75 |

Coordinator (`--to`) and worker (`--from`) names are 1-64 lowercase letters, digits, dots,
underscores or hyphens, starting with a letter or digit.

## Notice kinds, priority and coalescing

| Kind | Priority | Meaning |
|---|---|---|
| `blocked` | 1 | waiting on a permission dialog, a question, or other outside input |
| `exited` | 1 | the worker's session or pane ended |
| `idle` | 2 | finished a turn and waiting for input |
| `message` | 2 | an explicit message from the worker |
| `still-idle` | 3 | reminder that an already-reported idle worker is still idle |
| `progress` | 3 | new output while the worker keeps working |
| `working` | — | the worker resumed; stores nothing |

- **One live state notice per worker.** `blocked`, `exited`, `idle`, `still-idle` and `progress`
  are state notices. Posting one replaces the worker's current live state notice, keeps the
  original creation time, counts the replacement, and widens the transcript range when both name
  the same file. The latest state is the true one.
- **`working` withdraws** the worker's live state notice: the worker picked up work on its own,
  so the coordinator does not need to hear that it was idle. Messages stay.
- **Messages are never coalesced.** Give `--id KEY` to make a repeated post a no-op; the key is
  remembered through delivery, so a retry after delivery is also a no-op.
- **Stale reminders expire.** An undelivered priority-3 notice older than `--stale-after`
  (86400 seconds by default) is dropped when the queue is read. Priority 1 and 2 notices never
  expire.
- **Capacity.** A queue holds at most `--max-live` notices (200 by default). A post that would
  add a notice beyond that exits 75; a post that replaces an existing state notice is still
  accepted.
- Notice text is at most 4000 bytes. `--cursor PATH:START:END` records the transcript byte range
  the notice covers, so the coordinator can read the source instead of trusting a summary.

## Ordering and rendering

Delivery order is priority, then creation time. A batch starts with one header line, followed by
one block per notice. Each block shows the kind, worker, time, text cut at 600 bytes, and the
transcript range. Notices are added while the batch stays within `--max-bytes` (3000 bytes by
default). The first notice is always included, so one oversized notice cannot stall the queue.
Notices that do not fit stay queued, and the batch ends with a line saying how many.

`render` prints the next batch without claiming anything. `list` prints the queue as JSON.

## Delivery

`deliver` holds `.deliver.lock` for its whole run and follows these steps:

1. If a claimed batch exists from an earlier failed attempt, it is re-sent first, unchanged.
2. Otherwise the next batch is rendered, written to `claimed/`, and its notices leave `live/`.
3. The adapter runs. On success the batch moves to `delivered/`. On failure it stays in
   `claimed/`, `deliver` exits 69, and the next `deliver` re-sends the same batch.

Adapters (`--via`):

- `print` writes the batch to stdout and counts that as delivered. Use it from a cron job, a
  harness loop, or any reader that consumes stdout.
- `agentcloud-notify` runs
  `agentcloudctl notify --session ID --mode cli-script --idempotency-key agentctl-inbox-<coordinator>-<batch> --text BATCH`.
  The session receives the batch as a peer message from `cli-script`, even in the middle of a
  turn. Because a retried batch keeps its id, a retry after an ambiguous failure cannot deliver
  twice. Batches over 100000 bytes are refused.

An empty queue delivers nothing and exits 0.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success (including an empty delivery and a duplicate post) |
| 1 | a file could not be read or written, or a record is corrupt |
| 2 | invalid arguments |
| 69 | the delivery adapter failed; the batch stays claimed for a retry |
| 75 | busy: the queue is full, or another delivery is running |

## Examples

```bash
agentctl inbox post --to coord --from builder --kind progress --text 'tests 40/120' \
  --cursor /home/me/.claude/projects/p/abc.jsonl:1048576:1200000
agentctl inbox post --to coord --from builder --kind idle --text 'All 120 tests pass; pushed 3f2a9c1'
agentctl inbox post --to coord --from builder --kind message --id builder-final --text-file report.md
agentctl inbox list --to coord
agentctl inbox render --to coord --max-bytes 2000
agentctl inbox deliver --to coord --via print
```

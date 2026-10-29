# agent-turnover: session lineage and deterministic turnover for named agents

Design note for #686 agent-turnover. Date: 2026-10-29. Status: proposal, for the owner's review.

## The request

A named agent should have a list of harness sessions, not just one. A restart sometimes
resumes the same session. Sometimes the harness has a problem, and the work has to continue
in a fresh session from a handoff message. The owner asked two things:

- is there an advantage in agentctl knowing that the old session and its replacement are one
  agent?
- can agentctl move towards turning over any subagent deterministically: ask for a handoff
  (saying where it goes), start the replacement, and feed it the handoff?

## What exists today

- A record (`agents/<name>/agent.json`) holds one session: `native_session` (schema
  `agentctl-native-session/v1`), or `session_value`/`resume`. The record's `token` names its
  generation.
- `stop` and `revive` rename the whole agent directory to `archive/<name>-<token>/` (record,
  queue with `processed/`, output). The archive is therefore already one entry per finished
  generation, each with its session ID, but nothing links an entry to its successor.
- `revive` replays the launch spec and resumes the recorded conversation. `takeover` and
  `handback` move one session between the headless wrapper and an interactive tab. `restore`
  revives after a reboot and calls `wrkslots transfer-owner`. `rename` keeps `name_history`.
  None of them starts a fresh session on purpose.
- Records are strict. Nested v2/v3 rows accept only their exact key sets, and the Python
  edition must accept and refuse the same rows. **A lineage list cannot go into `agent.json`**
  without changing both editions' row contracts.

## What continuity buys

1. **Finding the past.** `agentctl history NAME` lists every session the name ran, so a
   transcript, a cost total or an event log can be found without guessing from timestamps.
   Today, once a session is replaced, its ID survives only inside an archive directory named
   by token.
2. **Accounting.** Usage per agent (`agent-usage`) and the watcher's progress history can
   sum across the agent's sessions instead of restarting at zero.
3. **Safety of session IDs.** A retired session stays bound to its agent, so no other agent
   can adopt or resume it by mistake. The existing `identity_owner` check covers only live
   records.
4. **A clean restart is a tool.** A fresh session drops a polluted context, a wedged harness
   and session-latched state. An example of the last: a guardrail's external-content taint,
   which is set per session and never cleared (#670 agentctl-console found that no prompt
   clears it). Turnover makes that restart routine and recorded, instead of a manual
   stop-start that loses the thread.

## Lineage store

```text
REGISTRY/lineage/agent.json             tripwire (a JSON string, like agents/agent.json)
REGISTRY/lineage/<name>/entries.jsonl   append-only, one JSON object per line
REGISTRY/lineage/<name>/turnover.json   the journal of a turnover in progress (absent otherwise)
REGISTRY/lineage/<name>/handoffs/       one file per turnover: <seq>.md (a collection, so its own directory)
```

The lineage lives outside `agents/<name>/` because that directory is archived and replaced at
every stop and revive, while the lineage must outlive generations. `lineage/` holds one
collection (names); each name's directory holds fixed files plus the `handoffs/` collection.
This follows the design rule.

Entry (one line; schema `agentctl-lineage/v1`):

```json
{"schema": "agentctl-lineage/v1", "seq": 7, "at": 1793300000.1, "kind": "generation",
 "line": "ln-3f9c…", "name": "cl-foo", "token": "…", "harness": "claude", "model": "…",
 "session": "8b9d…", "relation": "turnover", "predecessor": {"token": "…", "session": "10154…"},
 "handoff": {"path": "handoffs/7.md", "sha256": "…"}, "by": "agentctl 1.x"}
```

- `kind`: `generation` (a harness session began serving the name), `ended` (its end, with a
  reason: stopped, died, turned-over, revived), `renamed` (from/to), `note`.
- `relation`: how the generation relates to the one before it. `start` (a new line), `resume`
  (revive or restore of the same session), `fresh` (a revive whose transcript was gone),
  `turnover` (a fresh session fed a handoff), `takeover` and `handback` (same session, another
  front end).
- `line`: a lineage ID assigned at `start`, inherited by every later relation. A retired name
  reused for unrelated work gets a new `line`; `history` shows the current line and `--all`
  shows earlier ones.

Compatibility rules:

- **Advisory, never authority.** Routing, liveness and locks keep reading `agent.json` only. A
  failed lineage append after a successful start or revive is a warning, not a failure. History
  then fills the gap from the archive (below).
- **Forward:** readers ignore unknown keys and unknown `kind`s. A writer that finds a line with
  a schema major it does not know (`agentctl-lineage/v2`) refuses to append, fails closed and
  says so. `history` still prints such lines raw.
- **Backward:** builds older than this one never read or write `lineage/`. `lineage` joins the
  registry's reserved names. The tripwire `lineage/agent.json` is a JSON string, so an old
  build asked for an agent named `lineage` finds an unreadable record and refuses, rather than
  writing into it (the same trick as layout 2's `agents/agent.json`). A registry that already
  has an agent named `lineage` is refused before the directory is created.
- **Backfill:** `history` derives entries for generations no new build recorded (anything
  before this change, anything the Python edition or an old build revived) from
  `archive/<name>-<token>/agent.json` and the live record, marked `derived` with relation
  `unknown`. Nothing is rewritten. An explicit `agentctl lineage backfill NAME` (dry run by
  default) can write them down later if wanted.
- **Appends:** `O_APPEND` writes of one line under the name's lifecycle lock. A torn last line
  (a crash mid-write) is skipped on read and reported.

`agentctl history NAME [--all] [--json]` prints seq, time (ET in human output, UTC in JSON),
relation, session, token (short), harness and model, end reason, and handoff. `agentctl
history --session ID` answers which agent owned a session.

## Turnover

`agentctl turnover NAME [--handoff-timeout 15m] [--interrupt] [--handoff FILE] [--no-ask]`

The journal `lineage/<name>/turnover.json` records `phase`, a nonce, the old token and
session, the handoff path, and timestamps. Every phase is idempotent, and rerunning the same
command continues from the recorded phase.

| Phase | What happens | Leaving it |
|---|---|---|
| `requested` | Checks: the agent is local, running and not paused; no takeover; no other turnover; its harness can start a fresh session from the launch spec. Writes the journal. | `asking`, or `captured` when `--handoff FILE` or `--no-ask` |
| `asking` | Queues one prompt, via the normal send path with an `agentctl turnover` header: "write your handoff to PATH (what you were doing, its state, the commits and branch, what is next, open questions, open chat requests), end it with the line `<!-- agentctl-turnover NONCE -->`, then stop." PATH is `lineage/<name>/handoffs/<seq>.md`. The prompt waits for the turn boundary, unless `--interrupt`. | `captured` when PATH ends with the nonce line and the agent is idle (headless: `turn.completed`; pane: the usual idle check). On a timeout, see failures. |
| `captured` | Records the handoff's sha256. Pending queue items stay where they are. | `stopping` |
| `stopping` | Stops the old harness the way `stop` does, but publishes through revive's journaled two-step: the old directory and its queue go to the archive, and an `ended` entry is appended. | `starting` |
| `starting` | Launches from the launch spec with the session flags replaced by a new session ID (per harness: Claude Code `--session-id NEW` in place of `--resume`/`--session-id OLD`). Appends the `generation` entry with `relation: turnover`. Runs `wrkslots transfer-owner` for a slot agent, as restore does. Registers the new pane in other registries that name the agent (the chat bridge's), as restore does. | `feeding` |
| `feeding` | Queues, first: "[from agentctl turnover · predecessor: NAME session OLD] You are continuing the work of NAME…", then the handoff text inline (or its path when over 48 KiB), then the predecessor's first brief from its archived `processed/` queue when one exists. Then it moves the old queue's undelivered items into the new queue, in their original order, after that prompt. | `done`: journal removed, `history` shows the new line, and the coordinator gets one notice |

### Variants the issue asks for

- `--resume`: the same machine, but `starting` resumes the old session (revive-style) instead of
  a new one. It is for a healthy session whose front end, profile or host process needs
  replacing. Skips `asking` unless `--handoff-file` is given, and records `relation: resume`.
- `--profile P` (and so a different harness or model): `starting` launches from profile P
  instead of the recorded launch spec, keeping name, slot and cwd. A harness switch implies a
  fresh session, so `--resume` with a different harness is refused.
- `--handoff-file PATH` is the issue's name for `--handoff FILE` above.
- `history` links each session to its transcript where the harness keeps one: for Claude Code,
  the project transcript named by the session ID; for a headless agent, also its event-log
  generation in the archive.
- `agentctl status NAME` and the watcher show "turned over at T (reason)" from the last
  `ended`/`generation` pair.

The issue suggests the lineage live on the agent record, in a subdirectory once it grows. It
goes beside the record instead, under `REGISTRY/lineage/<name>/`, for the two reasons above:
the agent directory is replaced at every generation, and the strict row contracts leave no
room for it in `agent.json`.

### Failure modes

- **The agent never writes the handoff, or writes it without the nonce line.** At the timeout
  (default 15 minutes) turnover stops in `asking` and changes nothing. The agent keeps running,
  and the journal stays with `phase: asking, timed_out: true`. The operator then chooses:
  rerun the command (waits again), `--abort` (journal removed), or `--no-ask`. That last option
  writes a mechanical handoff: the first brief, the last assistant messages from the event log
  or transcript, the queue's pending items, and the slot's branch and `git status`. It is
  labelled as mechanical.
- **The harness is already dead or wedged** (the main reason to turn over): it cannot be
  asked. Use `--no-ask` or `--handoff FILE`, which start in `captured`.
- **A crash mid-turnover.** The journal names the phase. Before `stopping`, the old agent is
  untouched and `--abort` is safe. From `stopping` onwards, the revive journal guarantees the
  old directory is wholly archived or wholly live, and a rerun finishes the step. In `starting`,
  a launched harness with no `generation` entry is found the way revive finds one (pane and
  process proof) and is not launched twice. `--abort` after `stopping` is refused; recovery is
  forward only.
- **Messages arriving during turnover.** Until `stopping`, sends reach the old agent as usual,
  and it may mention them in its handoff. From `stopping` to `feeding`, there is no live
  record, so a send fails as "agent is turning over; retry", not "unknown agent", because new
  builds read the journal. The chat bridge and watcher retry in that case. Whatever was queued
  but undelivered moves to the new session after the handoff prompt, so the replacement reads
  the handoff first. An old build (a pinned bridge) gets "unknown agent" for that window,
  normally seconds. Upgrade the bridge before relying on turnover for the coordinator.
- **Two operators.** The name's lifecycle lock and the journal's existence serialize turnovers.
  A second `turnover` reports the first one's phase.

### Interactions

- **revive / restore:** unchanged behaviour. They append `generation` entries with `resume` (or
  `fresh`) and the same `line`. Restore can later offer turnover for an agent whose transcript
  is gone, instead of a silent fresh start.
- **takeover / handback:** `takeover` and `handback` entries; no new line or token. Turnover is
  refused during a takeover.
- **rename:** a `renamed` entry in both names' files; the new name continues the same `line`.
- **wrkslots:** the slot keeps its generation, checkouts and handoffs; only the owner moves to
  the new harness (`transfer-owner`, which requires the old owner to be dead: it is, after
  `stopping`). The agent's own wrkslots handoff is separate from the turnover handoff, so the
  turnover prompt does not ask for one.
- **Watcher:** reads the journal. It reports no "dead" or "idle without verdict" for an agent in
  `stopping`..`feeding`, and resets its per-agent timers at `done`. It sends one notice: "NAME
  turned over: session OLD → NEW, handoff lineage/NAME/handoffs/7.md". A turnover stuck in one
  phase for more than 30 minutes is reported like a stall.
- **Chat bridge:** it targets the agent by name, and its pane registration is refreshed in
  `starting`. Open chat requests survive because their request keys appear in the handoff, and
  the bridge's reply reminders key on the request, not the session. A coordinator turnover
  is the case to try last, after throwaway agents and workers.

## Pieces, in landing order

1. Lineage store, entries written by start, revive, restore, adopt, rename, stop, takeover and
   handback. `agentctl history` with archive backfill. Tests on throwaway `to-*` agents and fake
   harnesses.
2. `agentctl turnover` for headless agents (exact idle detection from the event log), with the
   journal and the failure tests: no handoff, timeout, a crash at each phase, messages during
   it.
3. Turnover for pane agents; watcher and bridge awareness.

## Open questions for the owner

1. Is the handoff location `REGISTRY/lineage/<name>/handoffs/<seq>.md` right? It is
   agentctl-owned and survives `/tmp` cleanup. `--handoff-dir` could override it.
2. When the agent does not answer: stop and leave it running (proposed), or fall back to the
   mechanical handoff automatically?
3. Should the replacement also get the predecessor's first brief (proposed: yes, after the
   handoff), or only the handoff?
4. Should `restore` turn over, rather than silently start fresh, when an agent's transcript is
   missing?

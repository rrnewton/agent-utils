# agentctl-team-messaging: messaging another team's coordinator, across hosts, with stale state in mind

Working design, 2026-10-24. Not user documentation, and nothing here is implemented.
`#496 agentctl-team-messaging`.

Code references are to `main` at `8efb7191`.

## 0. Summary

A **team** is a coordinator plus the workers it manages: one harness directory, one agentctl
registry, one Herdr workspace, on one host. The owner runs several teams on several hosts. Today
a team's coordinator can be reached from outside its harness only by someone who knows, by hand,
which host it is on, which registry names it, which binary drives that registry, and whether the
chat bridge that shares its queue is still the one running. That knowledge goes stale, and every
incident in section 1 is a delivery made on stale knowledge.

The proposal has three parts:

1. **A sparse teams registry**, one owner-level file (`teams.yaml`), saying which teams exist, the
   hosts they run on, where each coordinator entry point lives, an ordered list of contacts, and
   how to reach each host. It is a routing hint, not a mirror of Herdr: it is edited by people
   and by `agentctl teams discover --apply`, never by a send.
2. **Host-local resolution at delivery time.** The sender's file only has to get the message to
   the right host. On that host, `agentctl team accept` resolves the team from a small anchor
   file the harness itself keeps current (`REGISTRY/team.json`), re-verifies the coordinator
   (live record, pinned harness process, current boot, bridge unit), applies the receiving
   team's own policy, and enqueues into agentctl's durable queue. The sender never types and
   never trusts its own copy of the facts.
3. **Messages that know when they are stale.** Every team message carries an expiry, an optional
   topic, and optional `supersedes` keys. agentctl's queue drain checks them at the moment of
   typing, not at the moment of enqueueing, so a "hold" queued for an agent that only takes input
   when idle is discarded, with a receipt to the sender, instead of arriving hours late.

Sender attribution is derived and proved by agentctl, never claimed; nothing is ever sent as the
owner. Receipts come back by pull, which also works where only one host can reach the other.

## 1. Incidents this design must prevent

Each of these happened on the owner's hosts in October 2026. They are described here without
host or project names.

- **I1. Coordinator found by reading tabs.** An operator needed to message a team's coordinator
  and found it only by reading Herdr tab labels by hand: it was an agentcloud session viewed
  through `agentterm` in a Herdr tab, in no agentctl registry, and nothing on the host recorded
  that it was the coordinator.
- **I2. Two chat bridges watching one pane.** Two bridge states, each with its own channel,
  routed to the same coordinator pane. One bridge saw a reply block that belonged to the other,
  reported it to the agent as "not sent"; the agent sent it again, and the channel got
  duplicates. Two bridges on one pane are a supported arrangement (one per chat provider), but
  only when each has its own non-overlapping `reply_alias_range`
  (`rs/agentctl/src/embedded_chat_userguide.md:831-849`); that key was added after this
  incident, and nothing checks that a second bridge on a pane has one.
- **I3. Stale holds delivered late.** Several messages were queued for a Muse agent. agentctl
  types into a Muse pane only when its composer is idle and bare
  (`embedded_userguide.md:410-420`), and the agent stayed busy for hours. When it went idle the
  backlog drained in order, including a "hold" that a later message had already lifted. Nothing
  in the queue knew the hold had expired or been superseded.
- **I4. A contact naming a disabled bridge unit.** A per-host coordinators file named a bridge
  unit whose queue the coordinator shared. The unit had since been disabled. The delivering
  helper noticed only that the unit was "not running" and fell back to the file's own registry
  and binary paths, with a note, not a failure, so the configuration's claim that a bridge
  serves that queue stayed wrong and unflagged.

Two general lessons follow. First, every fact about another team is stale until re-checked on
that team's host, at delivery time. Second, a message's meaning can expire before it is
delivered, and only the delivering side knows when delivery happens.

## 2. What exists today (verified)

### 2.1 agentctl registry and queue

- **Registry.** `.agentctl/` in the harness directory by default (`--registry` overrides;
  `embedded_userguide.md:221`). One `REGISTRY/<agent>/agent.json` per agent, with Herdr
  `workspace_id`, `tab_id`, `pane_id`, `terminal_id`, `harness`, `lifecycle`, `slot`, a
  `token` (record generation) and, once pinned, `harness_identity` = `{boot_id, pid,
  starttime_ticks, executable_device, executable_inode}`. Read on this harness: the coordinator
  record is an adopted (`herdr-foreign`) Claude pane with both a foreign-shell and a harness
  identity. `profiles.json` sits beside the records. `REGISTRY/restore.json` names the
  coordinator and `also_registered` (other registries naming the same pane; used by `restore`,
  `embedded_userguide.md:578-582`).
- **Pane IDs are per Herdr server** (`w16:pD`). They are not a cross-host identity, and they
  change after a Herdr restart. The harness identity includes the boot ID, so a record from a
  previous boot is detectably stale.
- **Queue.** `REGISTRY/<agent>/queue/` has `target.json` (bound to a pane ID, never to a name)
  and four durable directories: `inbox/` (pending, not yet past the submission barrier),
  `inflight/`, `failed/` (quarantined, including `possibly_submitted`), `processed/` (working
  transition confirmed) (`embedded_userguide.md:951-968`). `send --message-id ID` refuses an ID
  already in any state (`cli.rs:645`). Uncertain delivery is never replayed.
- **Single typist.** Typing takes a host-wide target lock keyed by the pane ID
  (`subagents.rs:4275`, `agent.rs:1566`), so agentctl instances using different registries for
  the same pane serialize at the typing step. Nothing outside agentctl should type into a pane.
- **Readiness.** By default a send waits up to `--ready-timeout` (900 s) for an idle or done
  pane; `--deliver-while-working` lets Claude Code queue the prompt behind its turn. Muse is
  idle-only. There is **no** expiry, topic or supersede field on a queue item, and **no** bounded
  drain (no deadline, lock timeout or item limit on `send`/`drain`).
- **`agentctl inbox`** (`embedded_inbox_quickstart.md`) already has the superseding idea for
  worker notices: "A worker keeps one live state notice; a newer one replaces it", and
  priority-3 notices expire after `--stale-after` (default 86400 s, `inbox.rs:47`). It is a
  notice batcher for one coordinator on one host, not a transport.
- **agentcloud agents.** `agentctl send` to an agentcloud agent journals through
  `agentcloudctl send --end-of-turn`; when the caller is a different agentcloud session it uses
  an attributed `agentcloudctl send-message` instead, and an explicit human send from inside a
  different session is refused, "never disguised" (`cli.rs:139`).
- **No caller identity.** agentctl sets no environment variable naming the agent a process
  belongs to; a sender's identity has to be proved some other way (section 6.2).

### 2.2 wrkslots

`.wrkslots.yml` per harness describes the slot pool; slot ownership is per coordinator process.
It is local and plays no part in routing, but `discover` can read it to say which harness
directory a slot (and so a worker) belongs to.

### 2.3 A consuming project's per-host coordinators file

One consuming project keeps a file (schema `<project>/coordinators/v1`) that answers "where do
operational alerts go?" per host. Per host: a full name and optional ssh target, an optional
Herdr binary, and a `coordinator` with a `name`, `harness`, and ordered `contacts`:

- `agentcloud`: `session` UUID (and an informational URL); delivered with `agentcloudctl
  notify`, durable and idempotent by key.
- `agentctl`: `agent`, `registry`, `bin`, `herdr_bin`, optional `bridge_unit`, informational
  `workspace`/`tab`. Delivered with `agentctl send NAME --message-id ID`. When `bridge_unit` is
  running, the binary, registry and Herdr binary on its command line win and any difference is
  reported as config drift (local delivery only).
- `herdr`: refused since early October; typing into a pane is not done.

One helper delivers through it (about 3100 lines of Python). Beyond the contact walk it
carries hard-won rules worth keeping: an attempt ID stored before agentctl runs and reused on
every retry; outcome decided by where the item sits in the queue plus per-item delivery
evidence, not by exit status; `possibly_submitted` never retried and never resent under a new
ID until a human resolves it; a bounded run (40 + 10 s) that never kills agentctl; dedupe by key
with `--repeat-after`; and a JSONL log of every attempt. Cross-host delivery is ssh only, and
the bridge unit is not consulted remotely. When the named bridge unit is not running, it notes
that and uses the file's paths (incident I4).

### 2.4 agentcloud and host reach

- `agentcloudctl notify --session S --text T [--mode M] --idempotency-key K`: owner-only, into a
  session you own. Default `--mode system-notification` arrives as an **`[operator]`-labelled
  directive**; `--mode cli-script` arrives as a peer-level session message from "cli-script".
  Into a halted session it is receipted but never run, even after resume; check `inspect` for
  `halted: true` first.
- `agentcloudctl send-message --session SENDER --to TARGET --body B --idempotency-key K`:
  point-to-point from one session to another, same owner, arrives as ordinary input.
- `agentcloudctl inspect --session S`: folded state, including halted.
- A coordinator can be an agentcloud session viewed in a Herdr tab, registered in no agentctl
  registry (I1) unless someone ran `agentctl attach-self` or `adopt`.
- Hosts are reached by ssh, or with `agentcloudctl node exec --node NODE -- ARGV` where ssh is
  refused.

### 2.5 Chat bridges

`agentctl chat run` routes every thread of its channels to one agent in one registry and holds
an exclusive lease on its own state (`embedded_chat_userguide.md:270-290`). Two bridges cannot
split one channel. Two bridge states may target the same agent pane (one per provider) if
their `reply_alias_range`s do not overlap, but nothing checks that they do (I2).
Inbound admission is by `allowed_senders` and temporary grants. The bundled Slack provider
**ignores bot-message subtypes** (`embedded_chat_userguide.md:183`), so a message posted to a
team's channel by an app identity never reaches the agent through that bridge.

## 3. Model

```
owner-level teams.yaml (sparse, maybe stale)        on the target host (authoritative, fresh)
------------------------------------------         -------------------------------------------
team "tools"                                       $XDG_STATE_HOME/agentctl/teams.d/tools.json
  entry on host-a  --reach: local----------------->   -> /home/u/work/dev-tools/.agentctl
    contacts: agentctl coordinator@...              REGISTRY/team.json  (team, coordinator,
              agentcloud session ...                                     contacts, accepts_from)
              chat (visibility only)                REGISTRY/coordinator/agent.json (live identity)
team "sim"                                          bridge unit, if any (systemd state, cmdline)
  entry on host-b  --reach: ssh------------------->   ...
  entry on host-c  --reach: node-exec------------->   ...
```

- **Team.** A name (`^[a-z0-9-]{1,32}$`, like agent names). A team may have **entries** on
  several hosts (the consuming project in 2.3 runs one coordinator per host); `team@host` picks
  one, and `primary` picks the default.
- **Entry point.** The coordinator of one team on one host. Its contacts are tried in order.
- **Contact kinds.** `agentctl` (an agent in a registry on that host, optionally noting the chat
  bridge that shares its queue), `agentcloud` (a session), `chat` (a team channel; visibility
  only today, section 9). No `herdr` kind: nothing types into a pane except agentctl's drain.
- **Reach.** How the sender runs a command on the entry's host: `local`, `ssh` (target), or
  `node-exec` (node name), plus the agentctl binary to run there.
- **Anchor.** `REGISTRY/team.json` on the host, written by `agentctl team init` and kept current
  by the commands that change who the coordinator is (`rename`, `restore`, `adopt` of a
  coordinator). A host-local index `$XDG_STATE_HOME/agentctl/teams.d/<team>.json` points from a
  team name to its registry, so `team accept --team tools` needs no paths from the sender.

Why two levels: the owner's file changes when a team is created, moved to another host, or
retired: rarely, and by people. The anchor changes whenever a coordinator is revived, renamed or
re-adopted: often, and by agentctl itself, on the host where the change happens. Routing on the
first and resolving on the second means a stale owner file costs a warning, not a misdelivery.
Paths in `teams.yaml` remain as hints for hosts that predate anchors (phase 3 fallback).

## 4. The teams registry file

Default `${XDG_CONFIG_HOME:-~/.config}/agentctl/teams.yaml`; overridden by `--teams FILE` or
`AGENTCTL_TEAMS`. Mode 0600 is not required (it holds no secrets), but it is refused if writable
by others, since it routes messages. YAML because people edit it; `teams show --json` prints the
parsed form.

```yaml
schema: agentctl-teams/v1

hosts:
  host-a:
    reach: local
  host-b:
    reach: ssh
    ssh: host-b.example.net          # ssh target; default: the host key
    agentctl: ~/.local/bin/agentctl  # binary on THAT host; default: agentctl on its PATH
    herdr: ~/bin/herdr               # passed as --herdr-bin when set
  host-c:
    reach: node-exec                 # agentcloudctl node exec --node NODE -- ARGV
    node: host-c.example.net
    agentctl: ~/.local/bin/agentctl

teams:
  tools:
    description: develops the shared tooling
    primary: host-a
    entries:
      - host: host-a
        harness: ~/work/dev-tools     # hint: registry = <harness>/.agentctl unless set
        contacts:                     # tried in order (section 6.4)
          - kind: agentctl
            agent: coordinator
            registry: ~/work/dev-tools/.agentctl
            bridge:                   # the chat bridge that shares this queue, if any
              unit: agentctl-slack-dev-tools
              state: ~/.local/state/agentctl-slack-dev-tools/bridge
          - kind: chat                # visibility / last resort; never delivers to the agent today
            provider: slack
            channel: C00000000
    policy:                           # advisory here; the receiver's team.json is authoritative
      accepts_from: [sim, service:tick-hub]
    defaults:
      ttl: 2h                         # default expiry of messages TO this team
    reviewed: {at: 2026-10-24T12:00:00Z, by: "teams discover --apply"}

  sim:
    primary: host-b
    entries:
      - host: host-b
        harness: ~/work/dev-sim
        contacts:
          - kind: agentcloud
            session: 00000000-0000-0000-0000-000000000000
            expect_title: sim-coordinator  # identity check (section 6.3)
          - kind: agentctl
            agent: sim-coordinator
            registry: ~/.local/state/sim-chat/registry
      - host: host-c
        harness: ~/work/dev-sim
        contacts:
          - kind: agentctl
            agent: coordinator
    workers_on: [host-b, host-c]      # informational: where its slots run
```

**Freshness** is split from configuration on purpose:

- In the file: `reviewed` per team (when a person or `discover --apply` last confirmed it). The
  file is never rewritten by a send; a send that finds it wrong *proposes* an edit (prints the
  diff and `teams discover --host H --apply` to accept it).
- In state (`$XDG_STATE_HOME/agentctl/teams/observations.json`, machine-written): per contact,
  the last time it verified, the last delivery that confirmed, and the fingerprint seen then
  (`boot_id`, harness `pid`/`starttime_ticks`, `terminal_id`, record `token`, agentcloud session
  state). A changed fingerprint is reported ("coordinator re-launched since 3 h ago: new pid"),
  not refused: identity rests on the anchor and the live record, not on a process ID.
- `teams show` prints each entry's age: reviewed N days ago, last verified N minutes ago, last
  confirmed delivery N hours ago. Anything older than a threshold (default 7 days unverified) is
  marked `stale?` so a reader knows to run `teams check`.

### 4.1 The anchor, `REGISTRY/team.json`

```json
{
  "schema": "agentctl-team/v1",
  "team": "tools",
  "coordinator": "coordinator",
  "contacts": [
    {"kind": "agentctl", "agent": "coordinator",
     "bridge": {"unit": "agentctl-slack-dev-tools"}},
    {"kind": "agentcloud", "session": "00000000-0000-0000-0000-000000000000"}
  ],
  "accepts_from": ["sim", "service:tick-hub"],
  "limits": {"max_bytes": 8192, "per_sender_per_hour": 30},
  "default_ttl_seconds": 7200,
  "updated_at": "2026-10-24T12:00:00Z",
  "updated_by": "agentctl rename"
}
```

When `coordinator` is absent, `restore.json`'s `coordinator` is used. `rename` rewrites the
coordinator name in the same transaction as the record; `restore` and `adopt` refresh
`updated_at`. A registry with no anchor can still be reached by an explicit `agentctl` contact
(phase 3 fallback), and `discover` proposes `team init` for it.

## 5. Discovery and checking

`teams.yaml` is maintained by `discover`, which proposes, and by people, who accept.

```
agentctl teams discover [--host H]... [--apply] [--json]
agentctl teams check [TEAM[@HOST]]... [--json]
agentctl teams show [TEAM] [--json]
```

`discover` runs on each host named (local, or through its reach) a read-only probe,
`agentctl teams probe --json`, which reports:

1. **Registries.** From `teams.d/`, registries named in `teams.yaml` for that host, the
   `--registry` argument of every running `agentctl` process (bridges, watchers), and the
   `ExecStart` of user units whose command is agentctl. For each: `team.json`, `restore.json`'s
   coordinator, every record's lifecycle, pane, harness identity, and whether its boot ID is the
   host's current one.
2. **Herdr.** Workspaces, tabs and the agent or foreground program of each pane, marked with the
   registry (if any) that names the pane. A pane running a harness or `agentterm` that no
   registry names is reported as **unregistered** with the `agentctl adopt` or `attach-self`
   command that would register it (I1).
3. **agentcloud.** For each `agentterm -s SESSION` pane, the session ID and its `inspect` state
   (running, halted, archived).
4. **Chat bridges.** Every running `chat run` (by process), its bridge state, registry, agent,
   and the pane that agent resolves to; plus each configured bridge unit's `LoadState`,
   `UnitFileState` (`enabled`, `disabled`, `transient`, `not-found`) and `ActiveState`. Two
   running bridges resolving to one pane whose `reply_alias_range`s overlap, or where either has
   none, is a **shared-pane-bridge** finding (I2). A unit named
   in `teams.yaml` or `team.json` that is not found, disabled-and-inactive, or whose running
   command line names a different registry or agent is a **stale-bridge** finding (I4).

`discover` merges the probes into a proposed `teams.yaml` and prints a unified diff plus the
findings. `--apply` writes it atomically (after keeping the old file as
`teams.yaml.<timestamp>`), and stamps `reviewed`. It never runs `adopt`, `team init`, or any
`systemctl` verb; it prints them.

`check` is `discover` without the proposal: for each entry, the pre-delivery verification of
section 6.3 for every contact, as a table of contact, check, result and age. Exit 0 when every
entry has at least one verified contact, 1 otherwise. It is cheap enough for a timer.

## 6. Messaging

### 6.1 Command and envelope

```
agentctl team send TEAM[@HOST] (--file FILE | --text TEXT)
    [--key KEY] [--topic SLUG] [--supersedes KEY]... [--ttl DURATION | --expires TIME]
    [--kind info|request|hold|resume|reply] [--from-service NAME] [--contact N]
    [--wait SECONDS] [--json]
```

The sender builds an envelope and writes it to its outbox before any transport runs:

```json
{
  "schema": "agentctl-team-message/v1",
  "id": "tm-sim-6f1d0c2a9b7e4d31",
  "key": "ci-hold-2026-10-24",
  "from": {"team": "sim", "agent": "coordinator", "host": "host-b",
           "proof": "harness-ancestry"},
  "to": {"team": "tools", "host": "host-a"},
  "kind": "hold",
  "topic": "ci-hold",
  "supersedes": [],
  "created_at": "2026-10-24T13:00:00Z",
  "not_after": "2026-10-24T15:00:00Z",
  "reply_to": {"team": "sim", "host": "host-b", "in_reply_to": null},
  "body_sha256": "…",
  "body": "Please hold landing to main until the mirror is repaired."
}
```

- **ID.** `tm-<sender team>-` and 16 hex digits of SHA-256 over (sender team, key). With
  `--key`, a retry of the same command reuses the ID on every host and contact; without it the
  key is generated, stored in the outbox, and printed. The same key with a different body is
  refused as a conflict (stricter than `agentcloudctl notify`, where the same key silently keeps
  the first text); to change a message, send a new key that supersedes the old one.
- **Queue message ID** on the target is the envelope ID, so `agentctl send`'s existing "an ID
  already present in any delivery state is refused" makes `team accept` idempotent.
- **What is typed.** `team accept` renders the envelope on the receiving host:

  ```
  [team message tm-sim-6f1d0c2a9b7e4d31 from sim/coordinator@host-b (agent) to tools; kind hold; topic ci-hold; expires 15:00Z]
  Please hold landing to main until the mirror is repaired.
  [reply: agentctl team reply tm-sim-6f1d0c2a9b7e4d31 --file FILE]
  ```

  The header is generated by the receiver from the verified envelope. Body lines that begin with
  `[team message` or `[reply:` are indented by two spaces so a body cannot forge a header. The
  typed text never starts with `/`, so a body cannot become a harness slash command.

### 6.2 Who is sending

The sender is derived, then proved; a flag can only narrow it.

- **Agent in a registry** (`proof: harness-ancestry`): agentctl walks its own process ancestry
  and looks for a PID whose `(boot_id, pid, starttime_ticks)` equals some record's
  `harness_identity` in a registry on this host that has a `team.json`. That proves the caller
  runs inside that agent's harness. The team comes from that registry's anchor.
- **agentcloud session** (`proof: agentcloud-session`): `$AGENTCLOUD_SESSION_ID` matched against
  a team's `agentcloud` contact (or an `attach-self` record). Delivery to another agentcloud
  session then uses `send-message --session <self>`, which agentcloud attributes itself.
- **Service** (`proof: service`): timers and scripts pass `--from-service NAME`; the message is
  labelled `service:NAME` and is accepted only by teams whose `accepts_from` lists it.
- **Owner: never.** There is no `--from owner`; a process that cannot be proved to belong to an
  agent or a named service is refused, as `agentctl send` refuses a disguised human send today.
  For agentcloud targets this means team messages never use `notify`'s default
  `system-notification` mode, which renders as an `[operator]` directive; when the sender is not
  itself a session, `notify --mode cli-script` (peer-level) is used and the body carries the
  envelope header.

### 6.3 Verification before and after delivery

Every send re-checks, on the target host, that the contact is live and is who the file says.
Checks run inside `team accept` where the state is, except reach, which only the sender can
test.

| Check | agentctl contact | agentcloud contact | chat contact |
|---|---|---|---|
| Host reachable, agentctl present and new enough | `team accept --protocol 1` runs; an older binary is reported as `remote-too-old` | sender side | n/a |
| Team is who we think | `teams.d/<team>` or the hinted registry has `team.json` with `team: <team>` | `inspect`: session exists, owned, title or recorded team matches `expect_title` | channel matches the team's configured channel |
| Record is live | `agent.json` exists, `lifecycle: running`, not paused; record `boot_id` equals the host's current boot ID; the pinned harness process is alive with the same start ticks; pane resolves (agentctl's existing probe, `agentctl status`) | not `halted`, not archived | bridge unit active (if any) |
| Drainer exists | at least one of: a running bridge whose command line names this registry and agent; or the accept itself drains within its deadline; otherwise outcome `queued-no-drainer` | n/a | n/a |
| Bridge sanity | named bridge unit: `LoadState=loaded`, `UnitFileState` enabled or transient, active, and its `--registry` and agent equal the contact's. A mismatch, disabled or missing unit is a **stale-bridge** finding: the contact is still used if the record verifies, but the receipt carries the finding and the sender prints the proposed edit (I4: never a silent fallback). Two bridges on one pane without disjoint reply ranges: **shared-pane-bridge** finding (I2) | n/a | n/a |
| Policy | sender in `accepts_from`, size and rate limits | same, from the anchor if the session has one | same |

After the hand-off, the receipt says where the item is; the sender then re-reads it (immediately
and, with `--wait`, until it settles or the wait ends) through `team receipt ID` on the target.
agentctl's own around-the-effect checks (pinned process re-verified before Enter, prompt seen
leaving the composer) are what confirm delivery; `team send` reports them, it does not repeat
them.

### 6.4 Trying contacts in order

A contact is skipped to the next only on a **definite** refusal: a pre-delivery check failed, or
the target refused before enqueueing. Any uncertain outcome stops the walk: a transport that
died after handing the envelope over, `possibly_submitted`, or a receipt that cannot be read.
Trying the next contact then could deliver the same message twice through two routes, which is
I2 by another path. The sender prints which contacts were tried, why each was skipped, and what
to inspect.

### 6.5 Outbox, receipts and states

Sender state lives in `$XDG_STATE_HOME/agentctl/teams/`: `outbox/<id>.json` (envelope, attempts,
last receipt), `log.jsonl`, `observations.json`. Receiver state lives beside the target queue:
the queue item itself and `REGISTRY/team-log.jsonl`.

| State | Where the item is | Retried? |
|---|---|---|
| `accepted` | handed to the target's queue, not yet typed (`inbox/`) | the target's drainer types it |
| `delivered` | `processed/` with confirmed delivery evidence (the per-item rule the helper in 2.3 uses) | no |
| `expired` | `expired/`: `not_after` passed before it could be typed | no; sender told |
| `superseded` | `superseded/`: a later message with the same topic, or naming its key, arrived first | no; sender told which |
| `retracted` | `superseded/` by `team retract`, while still in `inbox/` | no |
| `possibly_submitted` | `inflight/` stranded or `failed/` uncertain | **never**; a human resolves it |
| `refused` | not enqueued: policy, verification, conflict | sender may fix and resend |
| `deferred` | still only in the sender's outbox (no contact reachable) | `team pump` retries until `not_after` |

`agentctl team outbox` lists open messages; `team status ID` refreshes one; `team pump` (for a
timer) refreshes every open receipt and retries `deferred` ones. Receipts are **pulled**: the
sender runs `team receipt` on the target through its reach. Nothing on the target has to reach
back, so a host that refuses inbound ssh can still send and learn the outcome.

### 6.6 Replies

`agentctl team reply ID --file FILE`, run inside the receiving team, builds an envelope to the
original `reply_to` with `in_reply_to: ID`, proved and policy-checked like any send. If the
replying host can reach the sender's host (its own `teams.yaml`, or the reach in `reply_to` when
that host is listed there), it is delivered directly. Otherwise it stays in the replying host's
outbox with state `awaiting-pull`, and the original sender's `team pump` collects replies
addressed to its team (`team pull --for sim`) over the reach it already uses. A reply is
delivered to the sender team's coordinator entry, not to whichever worker sent the original,
unless that agent is still live and named in `reply_to.agent`.

## 7. Agents that only take input when idle

I3 is not a transport failure; the message was delivered exactly as designed, hours after it
meant anything. The fix belongs in agentctl's queue, and it helps local sends too.

- **Expiry.** A queue item may carry `not_after` (Unix ms). The drain checks it under the queue
  lock immediately before typing (after readiness, which is where the waiting happens), and
  moves an expired item to a new `expired/` directory with a reason. Every team message has an
  expiry: `--ttl`/`--expires`, else the receiving team's `default_ttl_seconds`, else 2 h.
  `--ttl none` is allowed only for `--kind info` to a team that permits it.
- **Topics.** `--topic SLUG` makes the newest message in a topic, from the same sender team,
  supersede older ones still in `inbox/`: they move to `superseded/` with the superseding ID.
  This is `agentctl inbox`'s "one live state notice per worker" rule applied to a queue. A
  `resume` on topic `ci-hold` therefore cancels an undelivered `hold` on the same topic; if the
  hold was already delivered, the resume is delivered normally.
- **Explicit supersede.** `--supersedes KEY` cancels a specific earlier message (any topic) if it
  is still in `inbox/`. Too late is reported, not hidden: the receipt says the earlier one was
  already delivered.
- **Retract.** `agentctl team retract ID` asks the target to move the item to `superseded/` if
  it is still in `inbox/`.
- **Visible backlog.** `team accept` reports the target's queue depth and the age of its oldest
  pending item, and `team send` prints it ("3 messages already waiting, oldest 2 h; target is
  idle-only"). That is the signal I3 lacked.

Compatibility matters more than usual here, because an older drainer would type an expired item
without knowing it was expired. Every program that drains a queue (both agentctl editions, a
pinned chat bridge binary, any helper that runs `agentctl drain`) must honour the new fields
before any item carries them. So:

1. Queue items with `not_after`, `topic` or `supersedes` set `"queue_features": ["expiry"]`.
2. Both editions learn to honour and to refuse: a drainer that finds an item with a feature it
   does not know leaves it in `inbox/` and exits 75, instead of typing it.
3. Until every drainer on a host has step 2, `team accept` checks the binary of each running
   bridge for that registry (`agentctl capabilities` on that path) and, if one predates it,
   enqueues without expiry fields and returns a receipt warning `expiry-not-enforced`.

## 8. Safety

- **Single typist.** Team messaging adds no typing path. Its only effects are an agentctl queue
  item (typed later by agentctl's drain under the host-wide target lock) or an agentcloud journal
  entry. There is no `herdr` contact kind and no fallback to typing, matching the helper in 2.3.
- **No impersonation.** Sender identity is proved (6.2) and rendered by the receiver. The owner
  is never a sender. Body text cannot forge a header (6.1). Services are labelled as services.
  agentcloud deliveries never use operator-mode notifications.
- **Scoping.** The receiving team's `team.json` is authoritative: `accepts_from` (team names and
  `service:NAME`; default: nobody, so a team opts in), and optionally per-sender `kinds` (for
  example a service may send `info` but not `hold`). `limits` cap message size (default 8 KiB;
  agentcloud allows larger, but a coordinator's attention does not) and rate per sender per hour.
  A team may also declare `may_send_to` in its own anchor, checked by the sender, so a worker
  that wanders cannot message arbitrary teams. Workers send as their team, but `accepts_from`
  can name `sim/coordinator` to accept only a coordinator.
- **Threat model.** Every host runs as the same owner, and anything that can run `team accept`
  on a host could also write that host's queue files directly. The checks protect against
  confused and stale agents and scripts, not against a hostile process under the owner's
  account. That is why they are about provenance and freshness, not cryptography.
- **Logging.** Sender `log.jsonl` and receiver `team-log.jsonl` record ID, key, sender and its
  proof, target, contact, each check with its result, outcome, body SHA-256 and byte count. The
  body itself stays in the outbox and in the queue item, which are already private (0700
  registry), not in the log.

## 9. Chat bridges

- **A bridge as part of an `agentctl` contact.** Most coordinators that have a chat bridge share
  its queue: the bridge delivers owner messages into the same registry queue a team message uses.
  The `bridge` block on an `agentctl` contact records that, so checks can verify the bridge is
  the one the owner thinks is running (6.3). A team message is not a chat request: the bridge has
  no reply route for it, and the typed footer tells the coordinator to answer with `team reply`,
  not a fenced chat reply.
- **A `chat` contact.** Posting into a team's channel as the sending team's app identity. Today
  the Slack provider ignores bot-message subtypes (2.5), so this reaches the people in the
  channel, not the coordinator. That is still useful: as a last-resort contact ("no agent contact
  verified for team tools; owner, please look") and as an optional audit mirror
  (`--mirror-to-chat`). Making it a delivery path would need the bridge to admit specific app
  senders as team senders with distinct attribution, never folded into `allowed_senders`, which
  means the owner. It is never posted as the owner.
- **Bridges sharing a pane (I2).** The rule: every running bridge whose agent resolves to the
  same pane has its own `reply_alias_range`, and no two overlap. `discover` and `check` report
  violations; `team accept` reports one in the receipt. Separately (phase 5), `chat run` should
  refuse to start when another running bridge resolves to the same pane and either has no range
  or the ranges overlap. The deeper lesson of I2 is that a bridge reporting "not sent" makes an
  agent resend; a team message footer therefore never asks the agent to resend anything, and a
  team reply is idempotent by its own key.

## 10. Migration from a per-host coordinators file

The file in 2.3 maps onto `teams.yaml` as one team with one entry per host:

| coordinators/v1 | teams.yaml |
|---|---|
| `hosts.<short>` | `hosts.<short>` and an entry `{host: <short>}` in the project's team |
| `fqdn`, `ssh` | `reach: ssh`, `ssh: <ssh or fqdn>`; `reach: node-exec` added by hand where ssh is refused |
| `herdr` | `hosts.<short>.herdr` |
| `coordinator.name`, `harness` | informational on the entry (`coordinator`); the anchor is authoritative |
| contact `agentcloud` `session`, `url` | contact `agentcloud` `session`; URL dropped |
| contact `agentctl` `agent`, `registry` | contact `agentctl` `agent`, `registry` |
| `bin`, `herdr_bin` | `hosts.<short>.agentctl`, `herdr` (per host, not per contact) |
| `bridge_unit` | `bridge.unit` |
| `workspace`, `tab` | dropped; `discover` reports them live |
| contact `herdr` | not mapped; reported and dropped |

`agentctl teams import --coordinators-file FILE --team NAME` prints the converted entries for
review; it does not write.

The helper then becomes a thin wrapper: it keeps its command line, its dedupe and
`--repeat-after` memory, tick-report digests and `resolve`, and replaces its transport (contact
walk, bridge path resolution, ssh, queue settling) with

```
agentctl team send <team>@<host> --from-service tick-hub --key <key> --ttl <t> --kind info --file - --json
```

mapping receipt states onto its outcomes: `delivered` → delivered, `accepted` → queued,
`deferred` → deferred, `possibly_submitted` → possibly_submitted, `refused` → not-enqueued.
Its bounded run needs a bounded `team accept`; that is the same `--deadline`, lock timeout and
item limit the helper already asks agentctl for, and phase 3 adds them to `team accept` first.
Until a host's anchor exists, the wrapper's converted entries carry explicit registry hints, so
it works on day one.

## 11. CLI summary

```
agentctl teams show [TEAM] [--json]                        # parsed file + freshness
agentctl teams check [TEAM[@HOST]]... [--json]             # verify every contact; exit 1 if an entry has none
agentctl teams discover [--host H]... [--apply] [--json]   # probe hosts, propose a diff, findings
agentctl teams import --coordinators-file FILE --team NAME # convert, print only
agentctl teams probe --json                                # (runs on the target) read-only facts for discover

agentctl team init TEAM [--coordinator NAME]               # write REGISTRY/team.json and teams.d/TEAM
agentctl team send TEAM[@HOST] --file F [--key K] [--topic T] [--supersedes K]... [--ttl D] [--kind K] [--wait S] [--json]
agentctl team reply ID --file F
agentctl team retract ID
agentctl team status ID | outbox | pump | pull --for TEAM
agentctl team accept --protocol 1 [--team T] [--deadline S] --envelope -   # (runs on the target)
agentctl team receipt ID                                   # (runs on the target)

agentctl send NAME --file F [--expires TIME] [--topic T] [--supersedes ID]   # local sends gain phase-2 fields
```

Global: `--teams FILE` (default `$AGENTCTL_TEAMS`, then `$XDG_CONFIG_HOME/agentctl/teams.yaml`).
Exit status: 0 delivered or accepted; 75 deferred or queued with no drainer (retry later); 76
`possibly_submitted` (inspect, never resend); 1 refused; 2 usage or configuration.

## 12. Phased plan

1. **Read-only model (P1).** `teams.yaml` parser, `teams show`, `teams probe`, `teams check`,
   `teams discover` (proposal only), `teams import`. Addresses I1 (unregistered coordinator
   panes surface), I2 and I4 (shared-pane and stale bridges surface) with no change to delivery.
2. **Queue expiry (P2).** `not_after`, `topic`, `supersedes` on queue items in both editions,
   `expired/` and `superseded/` directories, the unknown-feature refusal, `agentctl send
   --expires/--topic/--supersedes`, and cross-edition differential cases. Addresses I3 for local
   sends immediately.
3. **Team delivery (P3).** `team init` and the anchor (plus `rename`/`restore` updating it),
   `team accept` with bounded deadline, `team send` over local, ssh and node-exec, outbox,
   receipts, `status`/`outbox`/`pump`/`retract`, sender proof.
4. **Replies and policy (P4).** `team reply`, `pull`, `accepts_from`/`may_send_to`, kinds, size
   and rate limits, both logs.
5. **Migration and bridges (P5).** The helper in 2.3 as a wrapper; `chat` contact (visibility and
   last resort); the shared-pane range check in `chat run`.

Phases 1 and 2 are independent and can proceed in parallel.

## 13. Questions for the owner

1. **Where does `teams.yaml` live across hosts?** One file on a primary host that other hosts
   never read (replies then depend on `reply_to` and pull), or a copy on every host kept in sync
   (by a dotfiles repository, or by `teams discover --apply` pushing it)? Recommendation: a copy
   on every host from a dotfiles repository; senders work from their local copy, and staleness is
   handled by the checks anyway.
2. **Default team for `team send proj` when a team has entries on several hosts:** the file's
   `primary`, the sender's own host if the team has an entry there, or refuse without `@host`?
   Recommendation: the sender's own host, then `primary`, then refuse.
3. **Default expiry.** Is 2 h right for messages without `--ttl`? Should idle-only targets (Muse)
   get a shorter default, and should `--ttl none` be allowed at all?
4. **Default scoping.** Should `accepts_from` default to nobody (each team opts in), or to every
   team in the owner's file? Recommendation: nobody, plus `service:` senders listed explicitly.
5. **Chat as a delivery path.** Do you want the bridge to admit specific app identities as team
   senders (so a post into a team's channel can reach its coordinator, visibly), or keep `chat`
   as visibility and last resort only?
6. **agentcloud identity.** For an agentcloud coordinator, is session title (`expect_title`) an
   acceptable identity check, or should `attach-self` registration (and so an anchor) be
   required before a session can be a team entry?
7. **Editions.** Team commands in the Rust edition only (the deployed bridges are Rust), with the
   Python edition refusing them clearly? The queue fields of phase 2 must land in both, since
   either edition may drain a queue.
8. **Workers as senders.** May a worker send to another team as its team, or only coordinators?
   Recommendation: only coordinators and named services by default; a team can allow its
   workers.

## 14. Related designs

Store-and-forward mail (an outbox, per-hop receipts, delivery status notifications) is the
closest analogy for the outbox and receipt states; message expiry here plays the role of a
message's delivery deadline. Matrix and XMPP federation route on a sparse address
(`user@server`) and leave authoritative state with the server that owns it, as `teams.yaml`
leaves it with the host's anchor. Message brokers with per-message TTL and "last value" topics
(for example NATS JetStream's per-subject limits, or Kafka log compaction by key) are the
precedent for topic superseding. None of them has the constraint that shapes this design: the
final hop is a terminal composer that only one program may type into, and only when the agent
behind it is idle.

# agentctl-team-messaging: messaging another team's coordinator, across hosts, with stale state in mind

Working design, 2026-10-24. Not user documentation, and nothing here is implemented.
`#496 agentctl-team-messaging`. Revised the same day after one adversarial review; section 15
lists what the review changed.

Code references are to `main` at `8efb7191`.

## 0. Summary

A **team** is a coordinator plus the workers it manages: one harness directory, one agentctl
registry, one Herdr workspace, on one host. The owner runs several teams on several hosts. Today
a team's coordinator can be reached from outside its harness only by someone who knows, by hand,
which host it is on, which registry names it, which binary drives that registry, and whether the
chat bridge that shares its queue is still the one running. That knowledge goes stale, and every
incident in section 1 is a delivery made on stale knowledge.

The proposal has four parts:

1. **A sparse teams registry**, one owner-level file (`teams.yaml`), saying which teams exist, the
   hosts they run on, where each coordinator entry point lives, an ordered list of contacts, and
   how to reach each host. It is a routing hint, not a mirror of Herdr: it is edited by people
   and by `agentctl teams discover --apply`, never by a send.
2. **Host-local resolution at delivery time.** The sender's file only has to get the message to
   the right host. On that host, `agentctl team accept` resolves the team from a small
   declaration the harness itself keeps current (`REGISTRY/team.json`), re-verifies the
   coordinator (live record, pinned harness process, current boot, bridge), applies the
   receiving team's own policy, records the message in a per-team ledger, and **enqueues** it
   into agentctl's durable queue. `team accept` never types; agentctl's existing drainers do,
   under the single-typist rule.
3. **Messages that know when they are stale.** Every team message carries an expiry, an optional
   topic ordered by the sender's clock, and optional `supersedes` keys. Cancellation is a
   tombstone that needs no lock, and the drain checks tombstones and expiry immediately before
   it pastes, so a "hold" queued for an agent that only takes input when idle is discarded, with
   a receipt to the sender, instead of arriving hours late.
4. **Attribution that is proved where it can be and labelled where it cannot.** Senders are
   derived from process ancestry or a registered service unit, never claimed by a flag, and
   nothing is ever sent as the owner. Receipts and replies come back by pull, which also works
   where only one host can reach the other.

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
  identity. `profiles.json` sits beside the records. `REGISTRY/restore.json`, when present,
  names the coordinator, the `units` to start and `also_registered` (other registries naming
  the same pane; `embedded_userguide.md:578-582`). It is optional, and absent on this harness.
  `stop` moves a record and its queue under `REGISTRY/archive/`, so a name reused later starts
  with an empty queue.
- **Pane IDs are per Herdr server** (`w16:pD`). They are not a cross-host identity, and they
  change after a Herdr restart. The harness identity includes the boot ID, so a record from a
  previous boot is detectably stale.
- **Queue.** `REGISTRY/<agent>/queue/` has `target.json` (bound to a pane ID, never to a name)
  and four durable directories: `inbox/` (pending, not yet past the submission barrier),
  `inflight/`, `failed/` (quarantined, including `possibly_submitted`), `processed/` (working
  transition confirmed) (`embedded_userguide.md:951-968`). `send --message-id ID` refuses an ID
  whose file exists in any phase (`cli.rs:645`); `withdraw` deletes an untyped item's file so
  its ID can be queued again (`agent.rs:1473-1480`), so the queue is not a permanent dedupe
  record. Uncertain delivery is never replayed. `processed/` is never pruned (201 items in the
  coordinator's queue here: 125 `chat-*`, 10 `agent-watch-*`, 2 `chat-reminder-*`).
- **Locks.** A drain takes the queue's `.delivery.lock` before its loop and holds it across
  every item, readiness waits included (`agent.rs:913-940`), and takes the host-wide target
  lock, keyed by the pane ID, before typing (`agent::lock_target`, `agent.rs:2476`; reached from
  `subagents.rs:4275`). Enqueue and `withdraw` need the same delivery lock. So while a drain
  waits up to `--ready-timeout` (900 s) for a busy pane, nothing can be added to or taken out of
  that queue. agentctl instances using different registries for the same pane serialize at the
  typing step. A bounded variant exists internally (`lock_target_within`, `agent.rs:2453`), but
  the CLI has no deadline, lock timeout or item limit on `send`/`drain`.
- **Readiness.** By default a send waits for an idle or done pane; `--deliver-while-working`
  lets Claude Code queue the prompt behind its turn, and Codex ordinary follow-ups are entered
  while working by default (`cli.rs:404`). Muse is idle-only. There is **no** expiry, topic or
  supersede field on a queue item.
- **Other queue writers.** The chat bridge queues `chat-*` prompts and reminders; `watch-agents`
  queues `agent-watch-*` notices into the coordinator's queue; a human or coordinator runs
  `agentctl send`. Every one of them drains the queue as it sends, oldest first, with whatever
  binary it was started with. On this harness the bridge and the watcher run two different
  pinned binaries. A `send_once` reminder refuses to queue when untyped prompts are already
  waiting (`subagents.rs:7880-7895`).
- **`agentctl inbox`** (`embedded_inbox_quickstart.md`) already has the superseding idea for
  worker notices: "A worker keeps one live state notice; a newer one replaces it", and
  priority-3 notices expire after `--stale-after` (default 86400 s, `inbox.rs:47`). Its
  `deliver --via agentcloud-notify` uses `agentcloudctl notify --mode cli-script` and never
  re-sends a claimed batch "to a different adapter or session" (`inbox.rs:248-250`). It is a
  notice batcher for one coordinator on one host, not a transport.
- **agentcloud agents.** `agentctl send` to an agentcloud agent journals through
  `agentcloudctl send --end-of-turn`; when the caller is a different agentcloud session it uses
  an attributed `agentcloudctl send-message` instead, and an explicit human send from inside a
  different session is refused, "never disguised" (`cli.rs:139`).
- **No caller identity.** agentctl sets no environment variable naming the agent a process
  belongs to; a sender's identity has to be proved some other way (section 6.2).

### 2.2 wrkslots

`.wrkslots.yml` per harness describes the slot pool; slot ownership is per coordinator process.
Slot boxes (cgroup, user-namespace or root isolation) limit what a worker can write, which
matters for where a sender's outbox can live (6.2). `discover` can read it to say which harness
directory a slot (and so a worker) belongs to.

### 2.3 A consuming project's per-host coordinators file

One consuming project keeps a file (schema `<project>/coordinators/v1`) that answers "where do
operational alerts go?" per host. Per host: a full name and optional ssh target, an optional
Herdr binary, and a `coordinator` with a `name`, `harness`, and ordered `contacts`:

- `agentcloud`: `session` UUID (and an informational URL); delivered with `agentcloudctl
  notify` in its default mode, so alerts arrive as `[operator]` system reminders, which the file
  calls safe mid-turn.
- `agentctl`: `agent`, `registry`, `bin`, `herdr_bin`, optional `bridge_unit`, informational
  `workspace`/`tab`. Delivered with `agentctl send NAME --message-id ID`. When `bridge_unit` is
  running, the binary, registry and Herdr binary on its command line win and any difference is
  reported as config drift (local delivery only).
- `herdr`: refused since early October; typing into a pane is not done.

One helper delivers through it (about 3100 lines of Python). Beyond the contact walk it
carries hard-won rules worth keeping:

- an attempt ID stored before agentctl runs and reused on every retry;
- outcome decided by where the item sits in the queue plus per-item delivery evidence, not by
  exit status; an item found in no phase is `lost`, never resent;
- `possibly_submitted` never retried and never resent under a new ID until a human resolves
  it, and "a change of the bridge's registry does not reopen it";
- **it defers instead of running agentctl whenever the target queue holds a chat prompt**,
  because `agentctl send`/`drain` would type that prompt without the bridge's
  read-before-typing check;
- a bounded run (40 + 10 s) that never kills agentctl, with a TODO asking agentctl for a
  deadline, a lock timeout and an item limit;
- dedupe by key with `--repeat-after`, and a JSONL log of every attempt.

Cross-host delivery is ssh only, with absolute or `~/` remote paths, and the bridge unit is not
consulted remotely. When the named bridge unit is not running, it notes that and uses the
file's paths (incident I4).

### 2.4 agentcloud and host reach

- `agentcloudctl notify --session S --text T [--mode M] --idempotency-key K`: owner-only, into a
  session you own. Default `--mode system-notification` arrives as an **`[operator]`-labelled
  directive**; `--mode cli-script` arrives as a peer-level session message from "cli-script".
  Into a halted session it is receipted but never run, even after resume; `inspect` reports
  `halted: true`. A keyless retry after a transport failure may deliver twice.
- `agentcloudctl send-message --session SENDER --to TARGET --body B --idempotency-key K`:
  point-to-point from one session to another, same owner, arrives as ordinary input; agentcloud
  itself attributes it to SENDER.
- `agentcloudctl inspect --session S`: folded state, including halted.
- A coordinator can be an agentcloud session viewed in a Herdr tab, registered in no agentctl
  registry (I1) unless someone ran `agentctl attach-self` or `adopt`.
- Hosts are reached by ssh, or, where ssh is refused, by a managed runtime's node-exec command
  that runs an argv on a named node.

### 2.5 Chat bridges

`agentctl chat run` routes every thread of its channels to one agent in one registry and holds
an exclusive lease on its own state (`embedded_chat_userguide.md:270-290`). Two bridges cannot
split one channel. Two bridge states may target the same agent pane (one per provider) if
their `reply_alias_range`s do not overlap, but nothing checks that they do (I2). The running
units here are `/bin/sh -c 'exec agentctl --registry R chat run --bridge-state S ...'`: after
the `exec`, the unit's main process is agentctl itself, its command line carries the registry
and bridge state but not the agent, which is in the bridge state's own configuration. Inbound
admission is by `allowed_senders` and temporary grants. The bundled Slack provider **ignores
bot-message subtypes** (`embedded_chat_userguide.md:183`), so a message posted to a team's
channel by an app identity never reaches the agent through that bridge. A bridge reads reply
markers on the agent's output rows, and in a read of an idle Claude Code pane longer than its
screen it can also count markers inside a received prompt (`embedded_chat_userguide.md:674-680`).

## 3. Model

```
owner-level teams.yaml (sparse, maybe stale)        on the target host (authoritative, fresh)
------------------------------------------         -------------------------------------------
team "tools"                                       $XDG_STATE_HOME/agentctl/teams.d/tools/
  entry on host-a  --reach: local----------------->   registry -> /home/u/work/dev-tools/.agentctl
    contacts: agentctl coordinator                    ledger/   (every message ever accepted)
              agentcloud session ...                REGISTRY/team.json  (declaration: team,
              chat (visibility only)                  coordinator, contacts, accepts_from)
team "sim"                                          REGISTRY/coordinator/agent.json (live identity)
  entry on host-b  --reach: ssh------------------->   bridge state and unit, if any
  entry on host-c  --reach: exec (template)------->   ...
```

- **Team.** A name (`^[a-z0-9-]{1,32}$`, like agent names). A team may have **entries** on
  several hosts (the consuming project in 2.3 runs one coordinator per host); `team@host` picks
  one.
- **Entry point.** The coordinator of one team on one host. Its contacts are tried in order.
- **Contact kinds.** `agentctl` (an agent in a registry on that host, optionally noting the chat
  bridge that shares its queue), `agentcloud` (a session), `chat` (a team channel; visibility
  only today, section 9). No `herdr` kind: nothing types into a pane except agentctl's drain.
- **Reach.** How the sender runs a command on the entry's host: `local`, `ssh` (target), or
  `exec` (an argv template, for a node-exec command), plus the absolute agentctl binary there.
- **Declaration.** `REGISTRY/team.json`, written by `agentctl team init` and kept current by
  the commands that change who the coordinator is (`rename`, `restore`, `adopt` of a
  coordinator). (Not called an "anchor": agentctl already uses that word for pinning a pane's
  harness process.) A host-local directory `$XDG_STATE_HOME/agentctl/teams.d/<team>/` holds a
  pointer to the declaring registry and the team's ledger, so `team accept --team tools` needs
  no paths from the sender, and the ledger survives the coordinator being stopped, renamed or
  replaced. Exactly one registry per host may declare a team: `team init` refuses when
  `teams.d/<team>` points at another registry whose declaration still names the team, and
  `team accept` refuses when it finds two.

Why two levels: the owner's file changes when a team is created, moved to another host, or
retired: rarely, and by people. The declaration changes whenever a coordinator is revived,
renamed or re-adopted: often, and by agentctl itself, on the host where the change happens.
Routing on the first and resolving on the second means a stale owner file costs a warning, not
a misdelivery. Registry paths in `teams.yaml` remain as hints for hosts with no declaration yet
(section 6.3, "undeclared").

## 4. The teams registry file

Default `${XDG_CONFIG_HOME:-~/.config}/agentctl/teams.yaml`; overridden by `--teams FILE` or
`AGENTCTL_TEAMS`. It holds no secrets but routes messages, so it is refused if writable by
others. YAML because people edit it; `teams show --json` prints the parsed form.

```yaml
schema: agentctl-teams/v1

hosts:
  host-a:
    reach: local
  host-b:
    reach: ssh
    ssh: host-b.example.net             # ssh target; default: the host key
    agentctl: ~/.local/bin/agentctl     # required for remote reach: absolute or ~/ on THAT host
    herdr: ~/bin/herdr                  # passed as --herdr-bin when set
    services:                           # services that may send from this host (6.2)
      tick-hub: {unit: tick-hub.service}
  host-c:
    reach: exec                         # argv template; {host} and {command} are substituted
    argv: [node-exec, --node, "{host}.example.net", --, sh, -c, "{command}"]
    agentctl: ~/.local/bin/agentctl
    stdin: false                        # this transport does not pass stdin (probed, 5)

teams:
  tools:
    description: develops the shared tooling
    primary: host-a
    entries:
      - host: host-a
        coordinator: coordinator        # informational; the declaration is authoritative
        harness: ~/work/dev-tools       # hint: registry = <harness>/.agentctl unless set
        contacts:                       # tried in order (section 6.4)
          - kind: agentctl
            agent: coordinator
            registry: ~/work/dev-tools/.agentctl
            bridges:                    # chat bridges that share this queue, if any
              - unit: agentctl-slack-dev-tools
                state: ~/.local/state/agentctl-slack-dev-tools/bridge
          - kind: chat                  # visibility / last resort; never delivers to the agent today
            provider: slack
            channel: C00000000
    policy:                             # advisory here; the receiver's declaration is authoritative
      accepts_from: [sim, service:tick-hub]
    defaults:
      ttl: 2h                           # default expiry of messages TO this team
    reviewed: {at: 2026-10-24T12:00:00Z, by: "teams discover --apply"}

  sim:
    primary: host-b
    entries:
      - host: host-b
        coordinator: sim-coordinator
        harness: ~/work/dev-sim
        contacts:
          - kind: agentcloud
            session: 00000000-0000-0000-0000-000000000000
            expect_title: sim-coordinator   # identity check (section 6.3)
          - kind: agentctl
            agent: sim-coordinator
            registry: ~/.local/state/sim-chat/registry
      - host: host-c
        coordinator: coordinator
        harness: ~/work/dev-sim
        contacts:
          - kind: agentctl
            agent: coordinator
    workers_on: [host-b, host-c]        # informational: where its slots run
```

**Freshness** is split from configuration on purpose:

- In the file: `reviewed` per team (when a person or `discover --apply` last confirmed it). The
  file is never rewritten by a send; a send that finds it wrong *proposes* an edit (prints the
  diff and `teams discover --host H --apply` to accept it).
- In state (`$XDG_STATE_HOME/agentctl/teams/observations.json`, machine-written): per contact,
  the last time it verified, the last delivery that confirmed, and the fingerprint seen then
  (`boot_id`, harness `pid`/`starttime_ticks`, `terminal_id`, record `token`, agentcloud session
  state). A changed fingerprint is reported ("coordinator re-launched since 3 h ago: new pid"),
  not refused: identity rests on the declaration and the live record, not on a process ID.
- `teams show` prints each entry's age: reviewed N days ago, last verified N minutes ago, last
  confirmed delivery N hours ago. Anything unverified for longer than a threshold (default 7
  days) is marked `stale?` so a reader knows to run `teams check`.

### 4.1 The declaration, `REGISTRY/team.json`

```json
{
  "schema": "agentctl-team/v1",
  "team": "tools",
  "coordinator": "coordinator",
  "contacts": [
    {"kind": "agentctl", "agent": "coordinator"},
    {"kind": "agentcloud", "session": "00000000-0000-0000-0000-000000000000"}
  ],
  "accepts_from": [
    {"sender": "sim/coordinator", "kinds": ["info", "request", "hold", "resume", "reply"]},
    {"sender": "service:tick-hub", "kinds": ["info"]}
  ],
  "may_send_to": ["sim"],
  "limits": {"max_bytes": 8192, "per_sender_per_hour": 30},
  "default_ttl_seconds": 7200,
  "updated_at": "2026-10-24T12:00:00Z",
  "updated_by": "agentctl rename"
}
```

When `coordinator` is absent, `restore.json`'s `coordinator` is used; with neither, `team init`
requires `--coordinator`. Bridges are not listed here: the receiver finds them from
`restore.json`'s `units` and from running `chat run` processes (5), so there is no third copy
of bridge unit names to go stale. `rename` rewrites the coordinator name in the same
transaction as the record; `restore` and `adopt` refresh `updated_at`.

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
   commands of user units that run agentctl. For each: `team.json`, `restore.json`'s
   coordinator and units, every record's lifecycle, pane, harness identity, and whether its boot
   ID is the host's current one.
2. **Herdr.** Workspaces, tabs and the agent or foreground program of each pane, marked with the
   registry (if any) that names the pane. A pane running a harness or `agentterm` that no
   registry names is reported as **unregistered** with the `agentctl adopt` or `attach-self`
   command that would register it (I1).
3. **agentcloud.** For each `agentterm -s SESSION` pane, the session ID and its `inspect` state
   (running, halted, archived).
4. **Chat bridges.** Every running `chat run`, found by process, read from
   `/proc/<pid>/cmdline` (registry, bridge state, binary) and its bridge state's configuration
   (agent, channels, `reply_alias_range`), and the pane that agent resolves to; plus, for each
   unit in `restore.json` or `teams.yaml`, `LoadState`, `UnitFileState` (`enabled`, `disabled`,
   `transient`, `not-found`), `ActiveState` and `MainPID`. Two running bridges resolving to one
   pane whose ranges overlap, or where either has none, is a **shared-pane-bridge** finding
   (I2). A unit that is not found, disabled and inactive, or whose main process names a
   different registry or agent, is a **stale-bridge** finding (I4).
5. **Transport.** The agentctl binary's path and capabilities (`agentctl capabilities`), and,
   for `exec` reach, whether stdin reached the probe (it sends a short known string).

`discover` merges the probes into a proposed `teams.yaml` and prints a unified diff plus the
findings. `--apply` writes it atomically (keeping the old file as `teams.yaml.<timestamp>`), and
stamps `reviewed`. It never runs `adopt`, `team init`, or any `systemctl` verb; it prints them.

`check` is `discover` without the proposal: for each entry, the pre-delivery verification of
section 6.3 for every contact, as a table of contact, check, result and age. Exit 0 when every
entry has at least one verified contact, 1 otherwise. It is cheap enough for a timer.

## 6. Messaging

### 6.1 Command and envelope

```
agentctl team send TEAM[@HOST] (--file FILE | --text TEXT)
    [--key KEY] [--topic SLUG] [--supersedes KEY]... [--ttl DURATION | --expires TIME]
    [--kind info|request|hold|resume] [--from-service NAME] [--wait SECONDS] [--json]
```

`team send` builds an envelope, proves the sender (6.2), writes the envelope to the outbox, and
then runs the transport (6.4). The envelope:

```json
{
  "schema": "agentctl-team-message/v1",
  "id": "tm-sim-6f1d0c2a9b7e4d31",
  "key": "ci-hold-2026-10-24",
  "body_sha256": "…",
  "from": {"team": "sim", "agent": "coordinator", "host": "host-b", "seq": 412,
           "proof": "harness-ancestry"},
  "to": {"team": "tools", "host": "host-a"},
  "kind": "hold",
  "topic": "ci-hold",
  "supersedes": [],
  "created_at": "2026-10-24T13:00:00.250Z",
  "not_after": "2026-10-24T15:00:00Z",
  "reply_to": {"team": "sim", "host": "host-b", "agent": "coordinator", "in_reply_to": null},
  "body": "Please hold landing to main until the mirror is repaired."
}
```

- **Key and ID.** The ID is `tm-<sender team>-` and 16 hex digits of SHA-256 over (sender
  team, key). Keys are scoped to the sending team, deliberately: the same key from two of the
  team's hosts names the same message, so the same body is deduplicated and a different body
  is a visible conflict. Without `--key`, the key is derived from (target team, kind, topic,
  body) and printed, so re-running an identical command is a retry, not a second message;
  sending the same text again on purpose needs a new `--key`. `from.seq` is a per-host counter
  kept in the sender's state, used only for topic order (7).
- **Same key, same body** returns the existing message's current state from the receiver's
  ledger (6.5), never "refused". **Same key, different body** is refused as a conflict
  (stricter than `agentcloudctl notify`, where the same key silently keeps the first text); to
  change a message, send a new key that supersedes the old one.
- **What is typed.** `team accept` renders the envelope on the receiving host:

  ```
  [team message tm-sim-6f1d0c2a9b7e4d31 from sim/coordinator@host-b (agent, proved on host-b) to tools; kind hold; topic ci-hold; expires 15:00Z]
  Please hold landing to main until the mirror is repaired.
  [reply: agentctl team reply tm-sim-6f1d0c2a9b7e4d31 --file FILE]
  ```

  The header is generated by the receiver from the verified envelope, including how the sender
  was proved (6.2). Body lines that begin with `[team message` or `[reply:`, and lines that
  look like a chat bridge's reply markers, are indented by two spaces, so a body can neither
  forge a header nor be posted by a bridge that reads a long idle pane (2.5). The typed text
  never starts with `/`, so a body cannot become a harness slash command.

### 6.2 Who is sending

The sender is derived, then proved; a flag can only narrow it.

- **Agent in a registry** (`proof: harness-ancestry`): agentctl walks its own process ancestry
  and looks for a PID whose `(boot_id, pid, starttime_ticks)` equals some record's
  `harness_identity` in a registry on this host that has a declaration. That proves the caller
  runs inside that agent's harness. The team comes from that declaration.
- **agentcloud session** (`proof: agentcloud-session`): `$AGENTCLOUD_SESSION_ID` is only an
  environment variable, so on its own it proves nothing. It counts as proof only for delivery
  to an agentcloud contact through `send-message --session <self>`, which agentcloud attributes
  itself. For any other contact it is labelled `asserted: agentcloud-session`.
- **Service** (`proof: service-unit`): `--from-service NAME` is accepted only when NAME is
  registered for this host in `teams.yaml` with a systemd unit, and `/proc/self/cgroup` shows
  the caller running inside that unit. It is refused outright when the ancestry walk finds an
  agent harness, so an agent refused under its own name cannot retry as a service.
- **Owner: never.** There is no `--from owner`; a process that cannot be proved to belong to an
  agent or a registered service is refused, as `agentctl send` refuses a disguised human send
  today. For agentcloud targets this means team messages never use `notify`'s default
  `system-notification` mode, which renders as an `[operator]` directive; when the sender is
  not itself a session, `notify --mode cli-script` (peer-level, as `inbox deliver` already uses)
  is used and the body carries the header.

**Proof is taken once, at outbox write, in the sender's own process tree.** Transport may run
elsewhere, for example in a Herdr command pane through `herdr-run` (required for network
commands on some hosts), where the ancestry ends at the Herdr server and proves nothing. So
`team send` records the proof in the outbox entry and `team pump` (any process, including a
`herdr-run` pane or a timer) carries it. For local reach, `team accept` re-derives the proof
itself from the outbox entry's recorded caller and refuses a mismatch. For remote reach the
receiver cannot re-derive anything; it records the transport (`ssh from host-b`, `exec`) and
renders the proof as "proved on host-b", which is an assertion by that host's agentctl. Section
8 says why that is acceptable.

**Isolation.** The outbox lives at `REGISTRY/team-outbox/` of the sender's own registry, which
is writable to an unboxed coordinator and to one in a wrkslots coordinator box. A worker in a
slot box (cgroup, user-namespace or root isolation) generally cannot write its coordinator's
registry, and its ancestry may stop at the box's relay; phase 3 does not support workers as
senders. They ask their coordinator, which is also the default policy (owner question 8). Each
supported mode gets a test that proves the sender from inside it.

### 6.3 Verification before and after delivery

Every send re-checks, on the target host, that the contact is live and is who the file says.
Checks run inside `team accept` where the state is, except reach, which only the sender can
test.

| Check | agentctl contact | agentcloud contact | chat contact |
|---|---|---|---|
| Host reachable, agentctl present and new enough | `team accept --protocol 1` runs; an older binary is a definite `remote-too-old` refusal | sender side | n/a |
| Team is who we think | `teams.d/<team>` points at a registry whose `team.json` says `team: <team>`, and no second registry claims it | `inspect`: session exists, owned, title matches `expect_title`, or the session has an `attach-self` record with a declaration | channel matches the team's configured channel |
| Record is live | `agent.json` exists, `lifecycle: running`, not paused; record `boot_id` equals the host's current boot ID; the pinned harness process is alive with the same start ticks; pane resolves (agentctl's existing probe, as `agentctl status`) | not `halted`, not archived, before and after the send (6.5) | n/a |
| Drainer exists | a running bridge on this registry and agent, or the host's `team drain` timer (7.3); otherwise the receipt says `queued-no-drainer` | n/a | n/a |
| Bridge sanity | each running bridge's main process names this registry and its bridge state names this agent; each unit in `restore.json`/`teams.yaml` is loaded, enabled or transient, and active. A mismatch, or a disabled or missing unit, is a **stale-bridge** finding: the contact is still used if the record verifies, but the receipt carries the finding and the sender prints the proposed edit (I4: never a silent fallback). Two bridges on one pane without disjoint reply ranges: **shared-pane-bridge** finding (I2) | n/a | n/a |
| Policy | sender and kind in `accepts_from`, size and rate limits, sender's `may_send_to` | same, from a declaration if the session has one; otherwise sender-side only, and the receipt says so | same |

**Undeclared targets.** Until a host has `team init`, an `agentctl` contact with an explicit
registry can still be used, but the "team is who we think" row cannot pass. Such a delivery is
accepted only with `--allow-undeclared` (the migration wrapper in section 10 passes it), the
receipt and both logs carry `unverified-team`, and the option is removed one release after
phase 5.

After the hand-off, the receipt says where the item is; the sender then re-reads it (at once
and, with `--wait`, until it settles or the wait ends) through `team receipt ID` on the target.
agentctl's own around-the-effect checks (pinned process re-verified before Enter, prompt seen
leaving the composer) are what confirm delivery; `team send` reports them, it does not repeat
them.

### 6.4 Trying contacts in order

The outbox entry records every attempt per contact. Two rules keep a message from arriving
twice by two routes, which is I2 by another path:

- **Advance only on a definite answer.** The walk moves to the next contact only after the
  previous one refused (a failed check, `remote-too-old`, policy), or after a later query of
  that contact's ledger (`team receipt ID`) shows the ID unknown there. A transport failure
  after the envelope may have been handed over (an ssh session that drops mid-command) is not a
  definite answer, and neither is an unreadable receipt.
- **Pin once handed over.** Once any contact has answered `accepted` or anything uncertain,
  the message is pinned to that contact; no later run, `team pump` included, tries another.
  This is the rule `inbox deliver` already has.

Connect failures that provably precede any remote command (name resolution, connection refused,
authentication refused before the session starts) are definite and recorded as such. The sender
prints which contacts were tried, why each was skipped, and what to inspect.

**How the envelope travels.** On stdin where the reach passes it (local, ssh, and an `exec`
template the probe confirmed), otherwise as one base64 argument, which the size limit keeps
small (8 KiB of body is about 11 KiB of argument).

### 6.5 Ledger, outbox, receipts and states

- **Sender:** `REGISTRY/team-outbox/<id>.json` (envelope, proof, attempts per contact, last
  receipt), `REGISTRY/team-log.jsonl`, and machine observations (4).
- **Receiver:** the team ledger, `$XDG_STATE_HOME/agentctl/teams.d/<team>/ledger/<id>.json`:
  body SHA-256, sender, registry and agent it was queued for, state, timestamps, and the newest
  `(created_at, host, seq)` seen per sender and topic. `team accept` writes the ledger entry
  before it enqueues and updates it after, so a crash between the two is recognised at the next
  accept or receipt. The ledger, not the queue, is the dedupe record: it outlives `withdraw`,
  `stop`, archive, `rename`, and a change of declaring registry, which keeps the helper's rule
  that a change of registry does not reopen a message.

| State | Where the item is | Retried? |
|---|---|---|
| `accepted` | in the target's queue, not yet typed | the target's drainer types it |
| `delivered` | `processed/` with confirmed delivery evidence (the per-item rule the helper in 2.3 uses) | no |
| `expired` | `not_after` passed before it could be typed | no; sender told |
| `superseded` | a newer message on its topic, or one naming its key, cancelled it before it was typed | no; sender told which |
| `retracted` | `team retract` cancelled it before it was typed | no |
| `possibly_submitted` | stranded in `inflight/` or uncertain in `failed/` | **never**; a human resolves it |
| `lost` | in the ledger as accepted but in no queue phase | **never**; reported until resolved |
| `refused` | not enqueued: policy, verification, conflict | sender may fix and resend under a new key |
| `deferred` | only in the sender's outbox (no contact gave a definite answer yet, or none was reachable) | `team pump` retries the pinned or next contact until `not_after` |
| `journaled` / `journaled-halted` | agentcloud contact: in the session's journal; `-halted` when `inspect` after the send shows the session halted, so it will not be run | no; `-halted` is reported as undelivered |

Receipts read the ledger and list queue directories without taking the delivery or lifecycle
lock, so `team receipt` answers at once even while a drainer waits on a busy pane.
`agentctl team outbox` lists open messages; `team status ID` refreshes one; `team pump` (for a
timer) refreshes every open receipt and retries `deferred` ones. Receipts are **pulled**: the
sender runs `team receipt` on the target through its reach. Nothing on the target has to reach
back, so a host that refuses inbound ssh can still send and learn the outcome.

**Retention.** Outbox entries and ledger entries in a terminal state are kept 30 days, then
pruned by `team pump`; `possibly_submitted` and `lost` are kept until resolved. The queue's own
`processed/` is outside this design (owner question 9).

### 6.6 Replies

`agentctl team reply ID --file FILE`, run inside the receiving team, builds an envelope to the
original `reply_to` with `in_reply_to: ID`, proved and policy-checked like any send. Its key
defaults to `reply:<ID>`, so re-running the same reply is a retry; a second, different reply to
the same message needs `--key`. If the replying host can reach the sender's host (its own
`teams.yaml`, or the reach in `reply_to` when that host is listed there), it is delivered
directly. Otherwise it stays in the replying team's outbox with state `awaiting-pull`, and the
original sender's `team pump` collects replies addressed to its team (`team pull --for sim`)
over the reach it already uses. A reply is delivered to the sender team's coordinator entry,
or to `reply_to.agent` when that agent is still live and named in the sender's declaration.

## 7. Agents that only take input when idle

I3 is not a transport failure; the message was delivered exactly as designed, hours after it
meant anything. The fix belongs in agentctl's queue, and it helps local sends too.

### 7.1 Expiry, topics, supersede, retract

- **Expiry.** A team queue item carries `not_after` (Unix ms). Every team message has one:
  `--ttl`/`--expires`, else the receiving team's `default_ttl_seconds`, else 2 h. `--ttl none`
  is allowed only for `--kind info` to a team whose declaration permits it.
- **Topics, by the sender's order.** `--topic SLUG` makes the newest message in a topic from
  one sender team supersede older ones from that team that have not been typed yet. "Newest" is
  the sender's `(created_at, host, seq)`, not arrival order: `team pump` can deliver a deferred
  message late, and a team with entries on two hosts can send from both. On arrival, a message
  older than the newest message the ledger has seen on its topic, **in any state, delivered
  included**, starts as `superseded` and is never queued. So a `hold` deferred at 13:00 and
  delivered at 13:10, after a `resume` created at 13:05 was typed, is dropped, not typed last.
  Within one sending host `seq` orders strictly; across hosts, `created_at` decides, and two
  messages within the skew tolerance (default 2 s) of each other on one topic from different
  hosts are both delivered, with a warning in both receipts. Local `agentctl send --topic` uses
  the topic namespace `local:<sender>`, so a local send never supersedes a team's message.
- **Explicit supersede.** `--supersedes KEY` cancels a specific earlier message (any topic) if
  it has not been typed. Too late is reported, not hidden: the receipt says the earlier one was
  already delivered.
- **Retract.** `agentctl team retract ID` cancels the message if it has not been typed.
- **Visible backlog.** `team accept` reports the target's queue depth and the age of its oldest
  pending item, and `team send` prints it ("3 messages already waiting, oldest 2 h; target is
  idle-only"). That is the signal I3 lacked.

### 7.2 Cancellation without the delivery lock

A drain holds the queue's delivery lock while it waits for a busy pane (2.1), which is exactly
the I3 window. If superseding needed that lock, a `resume` could not cancel a `hold` until the
drainer had already typed it. So cancellation is a **tombstone**: `queue/cancel/<id>.json`,
written by atomic rename with no queue lock (only the ledger's own short lock). The drain, for
each item that carries team fields, re-reads the item's tombstone and `not_after` **after**
readiness and immediately before the paste, under the target lock, and moves a cancelled or
expired item to a new terminal directory (`queue/cancelled/`, with the reason) instead of typing
it. A tombstone that arrives after the paste is too late, and the receipt says so. A new
message on a topic writes the tombstones for the older ones before it is itself enqueued; the
enqueue may then wait for the delivery lock (the drainer is busy), but the cancellation has
already taken effect.

### 7.3 Who drains, and never `team accept`

`team accept` enqueues and returns; it never types. Typing from `accept` would hold the delivery
and target locks across a readiness wait inside an ssh session that can drop (leaving a
`possibly_submitted` item), and would type any chat prompt ahead of it without the bridge's
read-before-typing check, which is the hazard the helper in 2.3 defers on.

Team items are typed by the drainers that already serve the queue (a bridge, `watch-agents`, a
human `send`), once those binaries understand them (7.4), or by a receiver-side `agentctl team
drain` run from a timer on hosts with no such drainer. `team drain` is bounded (`--deadline`,
`--lock-timeout`, `--max-items`: the flags the helper already asks agentctl for, which phase 3
adds as a prerequisite) and types only `tm-*` items, and only while the head of the queue is one:
if a chat prompt is ahead, it leaves the queue to the bridge and reports `waiting-behind-chat`.
`send_once` reminders (2.1) count queued team items like any other untyped prompt.

### 7.4 Older drainers must not see what they cannot honour

Old binaries ignore unknown JSON fields, so a marker field inside an item refuses nothing: any
pinned bridge, watcher, helper or rollback binary that drains the queue would type an expired
`hold`. So items with team fields are not written to `inbox/` at all, but to a new lane,
`queue/inbox.v2/`, which binaries before phase 2 never list. Phase-2 drainers merge `inbox/` and
`inbox.v2/` in enqueue order. Until a queue's drainers are new, team items in `inbox.v2/` are
typed only by `team drain`, and `team accept` says so in the receipt when it finds a running
drainer whose binary predates phase 2 (`agentctl capabilities` on that path).

Every consumer that hardcodes the four phases has to learn the new ones, or an expired or
cancelled ID looks absent:

- agentctl's enqueue duplicate check and `message_state` (`agent.rs:1422`, `1839`): both
  editions must list `inbox.v2/`, `cancel/` and `cancelled/`.
- The Python edition's chat runtime re-enqueues a `received` item it cannot find
  (`py/agentctl/chat_runtime.py:784-797`); it never owns `tm-*` IDs, but the listing must not
  treat the new directories as unknown.
- The helper in 2.3 classifies an item it cannot find as `lost`; it never owns `tm-*` IDs
  either, until it becomes the wrapper of section 10, which reads team receipts instead.

Local `agentctl send --expires/--topic/--supersedes` on ordinary (non-team) IDs is enabled only
after the same inventory covers every writer of that queue.

### 7.5 agentcloud contacts

No `team accept` runs for an agentcloud contact, so none of 7.1-7.3 applies there: agentcloud's
journal has no expiry or cancellation. The sender checks `not_after` and the ledger-free topic
rule from its own outbox immediately before the send, and refuses `--kind hold|resume` to an
agentcloud contact unless `--allow-unexpiring` is given. A `notify` arrives mid-turn as a system
reminder, so the I3 delay is shorter there, but it is not zero.

## 8. Safety

- **Single typist.** Team messaging adds no typing path. Its only effects are a queue item
  (typed later by agentctl's drain under the host-wide target lock) or an agentcloud journal
  entry. `team accept` never types (7.3). There is no `herdr` contact kind and no fallback to
  typing, matching the helper in 2.3.
- **No impersonation.** Sender identity is proved at outbox write (6.2), re-derived by the
  receiver for local reach, labelled as asserted by the sending host for remote reach, and
  rendered by the receiver. The owner is never a sender. Services are registered, verified by
  cgroup, and labelled as services. agentcloud deliveries never use operator-mode
  notifications. Body text cannot forge a header or a chat reply marker (6.1).
- **Scoping.** The receiving team's declaration is authoritative: `accepts_from` lists senders
  (`team`, `team/agent` or `service:NAME`) with the kinds each may send; the default is nobody,
  so a team opts in. `limits` cap message size (default 8 KiB; agentcloud allows more, a
  coordinator's attention does not) and rate per sender per hour. A team's own `may_send_to`,
  checked by the sender and again by the receiver, keeps a confused coordinator from messaging
  arbitrary teams.
- **Threat model.** Every host runs as the same owner, and anything that can run `team accept`
  on a host could also write that host's queue files directly. The checks protect against
  confused and stale agents and scripts, not against a hostile process under the owner's
  account. That is why they are about provenance and freshness, not cryptography, and why a
  remote proof asserted by the sending host's agentctl is acceptable, provided it is labelled.
- **Logging.** The sender's and receiver's `team-log.jsonl` record ID, key, sender and its
  proof, target, contact, each check with its result, outcome, body SHA-256 and byte count. The
  body itself stays in the outbox and in the queue item, which are already private (0700
  registry), not in the log.

## 9. Chat bridges

- **Bridges beside an `agentctl` contact.** Most coordinators that have a chat bridge share its
  queue: the bridge delivers owner messages into the same registry queue a team message uses.
  The `bridges` list on an `agentctl` contact in `teams.yaml` records which bridges the owner
  expects; the receiver checks them against what is actually running (6.3). A team message is
  not a chat request: the bridge has no reply route for it, and the typed footer tells the
  coordinator to answer with `team reply`, not a fenced chat reply.
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
  team reply is idempotent by its default key (6.6).

## 10. Migration from a per-host coordinators file

The file in 2.3 maps onto `teams.yaml` as one team with one entry per host:

| coordinators/v1 | teams.yaml |
|---|---|
| `hosts.<short>` | `hosts.<short>` and an entry `{host: <short>}` in the project's team |
| `fqdn`, `ssh` | `reach: ssh`, `ssh: <ssh or fqdn>`; `reach: exec` added by hand where ssh is refused |
| `herdr` | `hosts.<short>.herdr` |
| `coordinator.name` | the entry's `coordinator` (informational; the declaration is authoritative) |
| `coordinator.harness` | dropped; the record says which harness runs |
| contact `agentcloud` `session`, `url` | contact `agentcloud` `session`; URL dropped |
| contact `agentctl` `agent`, `registry` | contact `agentctl` `agent`, `registry` |
| `bin`, `herdr_bin` | `hosts.<short>.agentctl`, `herdr` (per host, not per contact) |
| `bridge_unit` | `bridges: [{unit: ...}]` on the contact |
| `workspace`, `tab` | dropped; `discover` reports them live |
| contact `herdr` | not mapped; reported and dropped |

`agentctl teams import --coordinators-file FILE --team NAME` prints the converted entries for
review; it does not write.

The helper then becomes a thin wrapper: it keeps its command line, its dedupe and
`--repeat-after` memory, tick-report digests and `resolve`, and replaces its transport (contact
walk, bridge path resolution, ssh, queue settling) with

```
agentctl team send <team>@<host> --from-service tick-hub --key <key> --ttl <t> --kind info \
    --allow-undeclared --file - --json
```

mapping receipt states onto its outcomes: `delivered` → delivered, `accepted` → queued,
`deferred` → deferred, `possibly_submitted` → possibly_submitted, `lost` → lost, `refused` →
not-enqueued, `expired`/`superseded` → expired. `--allow-undeclared` lets it work on hosts that
have no declaration yet. Its 50 s bounded run fits, because `team accept` only enqueues.

One behaviour changes and needs the owner's decision (question 6): its agentcloud alerts arrive
today as `[operator]` directives, and through `team send` they would arrive as peer-level
`cli-script` messages.

## 11. CLI summary

```
agentctl teams show [TEAM] [--json]                        # parsed file + freshness
agentctl teams check [TEAM[@HOST]]... [--json]             # verify every contact; exit 1 if an entry has none
agentctl teams discover [--host H]... [--apply] [--json]   # probe hosts, propose a diff, findings
agentctl teams import --coordinators-file FILE --team NAME # convert, print only
agentctl teams probe --json                                # (runs on the target) read-only facts for discover

agentctl team init TEAM [--coordinator NAME]               # write REGISTRY/team.json and teams.d/TEAM
agentctl team send TEAM[@HOST] --file F [--key K] [--topic T] [--supersedes K]... [--ttl D] [--kind K] [--wait S] [--json]
agentctl team reply ID --file F [--key K]
agentctl team retract ID
agentctl team status ID | outbox | pump | pull --for TEAM
agentctl team accept --protocol 1 [--team T] (--envelope - | --envelope-base64 B)   # (target) enqueue only
agentctl team receipt ID                                   # (target) lock-free read of the ledger and queue
agentctl team drain [--deadline S] [--lock-timeout S] [--max-items N]               # (target) types tm-* only

agentctl send NAME --file F [--expires TIME] [--topic T] [--supersedes ID]   # after 7.4's inventory
```

Global: `--teams FILE` (default `$AGENTCTL_TEAMS`, then `$XDG_CONFIG_HOME/agentctl/teams.yaml`).

Exit status follows `agentctl send`; the JSON `state` field is the full answer:

| Exit | Meaning | Safe to re-run the same command? |
|---|---|---|
| 0 | `delivered` | yes; it reports the same state |
| 75 | not settled: `accepted`, `deferred`, `awaiting-pull`, or `--wait` ended first | yes; the key makes it a retry |
| 76 | uncertain: `possibly_submitted`, `lost`, or a handover whose outcome is unknown | yes, it never resends; a human resolves |
| 65 | terminal and undelivered: `expired`, `superseded`, `retracted`, `refused`, `journaled-halted`, retract too late, or every contact refused (including `remote-too-old`) | yes; nothing new is sent |
| 2 | usage or configuration, including a team command run in an edition that lacks it | n/a |

## 12. Phased plan

1. **Read-only model (P1).** `teams.yaml` parser, `teams show`, `teams probe`, `teams check`,
   `teams discover` (proposal only), `teams import`. Addresses I1 (unregistered coordinator
   panes surface), I2 and I4 (shared-pane and stale bridges surface) with no change to delivery.
2. **Queue lanes and cancellation (P2).** `inbox.v2/`, `cancel/` tombstones, `cancelled/`,
   `not_after`, the merge in both editions, the pre-paste re-check, and the consumer inventory of
   7.4, with cross-edition differential cases (including an old drainer that must not see
   `inbox.v2/`, and a tombstone written while a drainer waits).
3. **Team delivery (P3).** Bounded drain flags (`--deadline`, `--lock-timeout`, `--max-items`) as
   a prerequisite; `team init` and the declaration (plus `rename`/`restore` updating it); the
   ledger; `team accept`, `team receipt`, `team drain`; `team send` over local, ssh and exec;
   outbox, pinned contact walk, `status`/`outbox`/`pump`/`retract`; sender proof for unboxed and
   coordinator-boxed senders, with a test per mode.
4. **Replies and policy (P4).** `team reply`, `pull`, `accepts_from` with kinds, `may_send_to`,
   size and rate limits, service registration, both logs, retention.
5. **Migration and bridges (P5).** The helper in 2.3 as a wrapper; `chat` contact (visibility and
   last resort); the shared-pane range check in `chat run`.

Phases 1 and 2 are independent and can proceed in parallel. Addressing I3 for ordinary local
sends waits for phase 2 plus the inventory.

## 13. Questions for the owner

1. **Where does `teams.yaml` live across hosts?** One file on a primary host that other hosts
   never read (replies then depend on `reply_to` and pull), or a copy on every host kept in sync
   (by a dotfiles repository, or by `teams discover --apply` pushing it)? Recommendation: a copy
   on every host from a dotfiles repository; senders work from their local copy, and staleness is
   handled by the checks anyway.
2. **Default entry for `team send proj` when a team has entries on several hosts:** the file's
   `primary`, the sender's own host if the team has an entry there, or refuse without `@host`?
   Recommendation: the sender's own host, then `primary`, then refuse.
3. **May `team accept` ever type?** The design says never (7.3), at the cost of needing a drainer
   on each receiving host: a bridge, a new enough watcher, or a `team drain` timer. The
   alternative is a bounded type-on-accept for hosts with none.
4. **Expiry and delivery while working.** Is 2 h right for messages without `--ttl`? Should
   idle-only targets (Muse) get a shorter default, and should `--ttl none` be allowed at all?
   For Claude Code targets, should a `hold` use `--deliver-while-working`, so it arrives behind
   the current turn instead of waiting for idle?
5. **Default scoping.** Should `accepts_from` default to nobody (each team opts in), or to every
   team in the owner's file? Recommendation: nobody, plus `service:` senders listed explicitly.
6. **Operator or peer for services.** Should a registered service's message to an agentcloud
   coordinator arrive as an `[operator]` directive, as the consuming project's alerts do today,
   or as a peer-level message like every other team message? Recommendation: peer-level, with
   an explicit per-service opt-in in the receiving team's declaration if you want operator mode.
7. **agentcloud identity.** For an agentcloud coordinator, is session title (`expect_title`) an
   acceptable identity check, or should `attach-self` registration (and so a declaration) be
   required before a session can be a team entry?
8. **Workers as senders.** May a worker send to another team as its team, or only coordinators
   and registered services? Recommendation: only coordinators and services; workers in slot
   boxes cannot be proved or write an outbox in phase 3 anyway (6.2).
9. **Retention.** 30 days for terminal outbox and ledger entries? And should this work also
   prune the queue's `processed/` directory, which nothing prunes today?
10. **Clock skew.** Is a 2 s cross-host tolerance for topic order acceptable, or should cross-host
    senders of one team be required to share one sending host for topics?
11. **Editions.** Team commands in the Rust edition only (the deployed bridges are Rust), with
    the Python edition exiting 2 with a pointer? The queue changes of phase 2 must land in both,
    since either edition may drain a queue.

## 14. Related designs

Store-and-forward mail (an outbox, per-hop receipts, delivery status notifications) is the
closest analogy for the outbox and receipt states; message expiry here plays the role of a
message's delivery deadline. Matrix and XMPP federation route on a sparse address
(`user@server`) and leave authoritative state with the server that owns it, as `teams.yaml`
leaves it with the host's declaration. Message brokers with per-message TTL and "last value"
topics (for example NATS JetStream's per-subject limits, or Kafka log compaction by key) are the
precedent for topic superseding. None of them has the constraint that shapes this design: the
final hop is a terminal composer that only one program may type into, and only when the agent
behind it is idle.

## 15. Review record

One adversarial review (2026-10-24) of the first draft. Every cited line was re-checked; the
changes it caused:

- `team accept` no longer drains (it held the delivery and target locks across a readiness wait
  in an ssh session, and would type chat prompts without the bridge's check); 7.3.
- Cancellation became a lock-free tombstone re-checked before the paste, because the delivery
  lock is held through the very wait I3 is about; 7.2.
- Team items moved to a lane old drainers never list, and the consumers that hardcode queue
  phases are inventoried; 7.4. The first draft only checked running bridges.
- Topic order follows the sender's clock, and a late older message starts superseded; 7.1.
- A per-team receiver ledger replaced the queue as the dedupe record, adding `lost`; 6.5.
- `--from-service` requires a registered unit and cgroup membership, `$AGENTCLOUD_SESSION_ID`
  is an assertion, proof is taken at outbox write, and remote proofs are labelled; 6.2.
- The contact walk advances only on a definite answer and pins once handed over; 6.4.
- Keyless keys are derived from content, same-key-same-body returns the current state, replies
  default to `reply:<ID>`; 6.1, 6.6.
- agentcloud contacts got their own states and limits; 6.5, 7.5.
- The bridge check reads the main process and the bridge state, and reuses `restore.json`;
  2.5, 5.
- The local "anchor" was renamed "declaration" (agentctl already uses "anchor"); two registries
  claiming a team are refused; undeclared targets are labelled and time-limited; 3, 6.3.
- Remote binaries must be absolute paths, the envelope falls back to an argument where stdin
  does not pass, the generic `exec` reach replaces a named node-exec command, exit codes were
  realigned with `agentctl send`, and owner questions 3, 4 (second half), 6, 9 and 10 were added.

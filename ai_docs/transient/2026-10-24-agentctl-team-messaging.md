# agentctl-team-messaging: messaging another team's coordinator, across hosts, exactly once

Working design, 2026-10-24. Not user documentation, and nothing here is implemented.
`#496 agentctl-team-messaging`.

**Revision 2** (same day) follows the owner's feedback on revision 1: the data files are JSON;
only coordinators (and registered services) send to other teams; routes are a pluggable layer
described in data (local, ssh with an optional managed link, a generic command transport for
company-internal channels, and an opportunistic chat relay); exactly-once delivery is a
first-class section; only the receiving host's own agentctl touches its panes; and team
messaging is factored as its own tool. Section 20 records what each revision changed.

Code references are to `main` at `8efb7191` unless marked with `2a9d8012`.

## 0. Summary

A **team** is a coordinator plus the workers it manages: one harness directory, one agentctl
registry (if it uses agentctl), one Herdr workspace, on one host. The owner runs several teams
on several hosts. Today a team's coordinator can be reached from outside its harness only by
someone who knows, by hand, which host it is on, which registry names it, which binary drives
that registry, and whether the chat bridge that shares its queue is still the one running. That
knowledge goes stale, and every incident in section 1 is a delivery made on stale knowledge or a
retry that delivered twice.

The proposal, in seven parts:

1. **A separate, à la carte tool** (working name `agent-teams`), not more subcommands of
   agentctl. It shares one small library with agentctl (queue format, ledger primitives,
   harness identity) and integrates with agentctl, chat bridges and transports through data and
   commands, so each piece works alone (section 4).
2. **A sparse teams file** (`teams.json`), one owner-level routing hint: which teams exist, on
   which hosts, their contacts, and the ordered routes to each host. Private route definitions
   live in separate data files (`routes.d/*.json`) so the public code carries no
   company-specific knowledge (sections 6, 10).
3. **The receiving host's agentctl does the work.** No route types into, reads, or drives a
   remote host's Herdr panes. Every route ends at the receiving host's own `agent-teams accept`,
   which verifies the coordinator there, applies that team's policy, records the message in the
   team's ledger and enqueues it; that host's local drainer types it (section 3).
4. **Coordinators only.** A message is sent by a team's coordinator (proved by process
   ancestry) or by a registered service, and is addressed to a team's coordinator. Workers
   neither send nor receive team messages (section 8.2).
5. **Exactly once.** One message ID, fixed at send and carried on every route; a durable
   per-team seen-set at the receiver, claimed atomically before enqueue; a second route or
   contact is tried only after the first is proved not to have delivered; ambiguity is resolved
   by asking the receiver, never by resending blind; and the drain never pastes one ID twice
   (section 9).
6. **Messages that know when they are stale.** Expiry, topics ordered by the sender's clock,
   supersede and retract, as lock-free tombstones re-checked just before the paste (section 12).
7. **Routes as data.** Each route names a driver built into the code (`local`, `ssh`,
   `command`) and is otherwise described in data: targets, keys, command templates, capability
   flags, idempotency semantics, attribution. A chat relay is opportunistic and never required
   (sections 10, 11).

## 1. Incidents this design must prevent

Each of these happened on the owner's hosts in October 2026. They are described here without
host or project names.

- **I1. Coordinator found by reading tabs.** An operator needed to message a team's coordinator
  and found it only by reading Herdr tab labels by hand: it was an agentcloud session viewed
  through `agentterm` in a Herdr tab, in no agentctl registry, and nothing on the host recorded
  that it was the coordinator.
- **I2. Two chat bridges watching one pane: duplicate replies.** Two bridge states, each with
  its own channel, routed to the same coordinator pane. One bridge saw a reply block that
  belonged to the other and reported it to the agent as "not sent"; the agent sent it again,
  and the channel got duplicates. Two bridges on one pane are a supported arrangement (one per
  chat provider), but only when each has its own non-overlapping `reply_alias_range`
  (`rs/agentctl/src/embedded_chat_userguide.md:831-849`); that key was added after this
  incident, and nothing checks that a second bridge on a pane has one.
- **I3. Stale holds delivered late.** Several messages were queued for a Muse agent. agentctl
  types into a Muse pane only when its composer is idle and bare
  (`embedded_userguide.md:410-420`), and the agent stayed busy for hours. When it went idle the
  backlog drained in order, including a "hold" that a later message had already lifted.
- **I4. A contact naming a disabled bridge unit.** A per-host coordinators file named a bridge
  unit whose queue the coordinator shared. The unit had since been disabled. The delivering
  helper noticed only that the unit was "not running" and fell back to the file's own paths,
  with a note, not a failure, so the configuration's claim stayed wrong and unflagged.
- **I5. A retry posted a message three times** (`#448 send-dedupe`). A long message, cut into
  three parts, took 7.5-12 s per attempt to post. The sending page showed the first attempt as
  failed while the server was still posting; the owner pressed Retry, and the whole message
  reached the channel three times (three requests, each answered 200; a two-times repeat the day
  before). The provider takes no idempotency key and the page sent none, so nothing could
  recognise the retry. The fix gave every composed message one key, carried on every attempt,
  and an outcome query to ask after a lost answer before calling a send failed.

Three lessons follow. Every fact about another team is stale until re-checked on that team's
host, at delivery time. A message's meaning can expire before it is delivered, and only the
delivering side knows when delivery happens. And a retry after an unclear outcome is a second
message unless the receiver can recognise it: I2 and I5 are both an unclear "did it go?"
answered by sending again.

## 2. What exists today (verified)

### 2.1 agentctl registry and queue

- **Registry.** `.agentctl/` in the harness directory by default (`--registry` overrides;
  `embedded_userguide.md:221`). One `REGISTRY/<agent>/agent.json` per agent, with Herdr
  `workspace_id`, `tab_id`, `pane_id`, `terminal_id`, `harness`, `lifecycle`, `slot`, a
  `token` (record generation) and, once pinned, `harness_identity` = `{boot_id, pid,
  starttime_ticks, executable_device, executable_inode}`. `profiles.json` (owner-edited launch
  profiles) sits beside the records; `REGISTRY/restore.json`, when present, names the
  coordinator, the `units` to start and `also_registered` (`embedded_userguide.md:578-582`).
  A chat bridge is configured by an owner-edited `chat.json` and keeps `bridge.json` in its
  state. **Every piece of agentctl state, owner-edited or machine-written, is JSON.**
  `stop` moves a record and its queue under `REGISTRY/archive/`.
- **Pane IDs are per Herdr server** (`w16:pD`): not a cross-host identity, and they change after
  a Herdr restart. The harness identity includes the boot ID, so a record from a previous boot
  is detectably stale.
- **Queue.** `REGISTRY/<agent>/queue/` has `target.json` (bound to a pane ID, never to a name)
  and four durable directories: `inbox/`, `inflight/`, `failed/` (quarantined, including
  `possibly_submitted`), `processed/` (`embedded_userguide.md:951-968`). `send --message-id ID`
  refuses an ID whose file exists in any phase (`cli.rs:645`); `withdraw` deletes an untyped
  item's file so its ID can be queued again (`agent.rs:1473-1480`), so the queue is not a
  permanent dedupe record. Uncertain delivery is never replayed. `processed/` is never pruned
  (201 items in this harness coordinator's queue on 2026-10-24).
- **Locks.** A drain holds the queue's `.delivery.lock` across every item, readiness waits
  included (`agent.rs:913-940`), and takes the host-wide target lock, keyed by pane ID, before
  typing (`agent.rs:2476`). Enqueue and `withdraw` need the same delivery lock, so while a drain
  waits up to `--ready-timeout` (900 s) for a busy pane, nothing can be added or removed. The
  CLI has no deadline, lock timeout or item limit on `send`/`drain`.
- **Readiness.** By default a send waits for an idle or done pane; `--deliver-while-working`
  lets Claude Code queue the prompt behind its turn (`cli.rs:404`). Muse is idle-only. There is
  **no** expiry, topic or supersede field on a queue item.
- **Other queue writers.** The chat bridge (`chat-*` prompts and reminders), `watch-agents`
  (`agent-watch-*`), and humans or coordinators (`agentctl send`). Each drains as it sends,
  oldest first, with whatever binary it was started with; here the bridge and the watcher run
  two different pinned binaries. A `send_once` reminder refuses to queue when untyped prompts
  are waiting (`subagents.rs:7880-7895`).
- **`agentctl inbox`** supersedes worker notices ("a newer one replaces it") and expires
  priority-3 notices after `--stale-after` (`inbox.rs:47`). Its `deliver --via
  agentcloud-notify` never re-sends a claimed batch "to a different adapter or session"
  (`inbox.rs:248-250`). It is a notice batcher for one coordinator on one host, not a
  transport.
- **agentcloud agents.** `agentctl send` to an agentcloud agent journals through `agentcloudctl
  send --end-of-turn`, or an attributed `send-message` when the caller is another session; a
  disguised human send from inside a session is refused (`cli.rs:139`). `attach-self` registers
  the caller's own session in a registry (`embedded_userguide.md:1175-1190`).
- **No caller identity variable.** agentctl sets no environment variable naming the agent a
  process belongs to; identity has to be proved (section 8.2).
- **No ssh.** agentctl itself never runs ssh today.

### 2.2 wrkslots

`.wrkslots.yml` per harness describes the slot pool; slot ownership is per coordinator process.
Slot boxes (cgroup, user-namespace or root isolation) limit what a worker can write, and its
process ancestry may stop at the box's relay.

### 2.3 A consuming project's per-host coordinators file

One consuming project keeps a file (schema `<project>/coordinators/v1`) that answers "where do
operational alerts go?" per host: a name and optional ssh target, an optional Herdr binary, and
a `coordinator` with ordered `contacts`: `agentcloud` (a session, delivered with `agentcloudctl
notify` in its default mode, so alerts arrive as `[operator]` system reminders), `agentctl`
(`agent`, `registry`, `bin`, `herdr_bin`, optional `bridge_unit`), and `herdr` (refused since
early October). One helper (about 3100 lines of Python) delivers through it, with rules worth
keeping:

- an attempt ID stored before agentctl runs and reused on every retry;
- outcome decided by where the item sits in the queue plus per-item delivery evidence, not by
  exit status; an item found in no phase is `lost`, never resent;
- `possibly_submitted` never retried or resent under a new ID until a human resolves it, and "a
  change of the bridge's registry does not reopen it";
- it defers instead of running agentctl whenever the target queue holds a chat prompt, because
  `send`/`drain` would type that prompt without the bridge's read-before-typing check;
- a bounded run (40 + 10 s) that never kills agentctl;
- dedupe by key with `--repeat-after`, and a JSONL log of every attempt.

Cross-host delivery is ssh only, using whatever keys the account has, and the bridge unit is not
consulted remotely (I4).

### 2.4 agentcloud and host reach

- `agentcloudctl notify --session S --text T [--mode M] --idempotency-key K`: owner-only. The
  default `system-notification` mode arrives as an **`[operator]`-labelled directive**;
  `--mode cli-script` arrives as a peer-level message. Into a halted session it is receipted but
  never run. A keyless retry after a transport failure may deliver twice; the same key with a
  different text silently keeps the first.
- `agentcloudctl send-message --session SENDER --to TARGET --body B --idempotency-key K`:
  session to session, same owner; agentcloud attributes it to SENDER.
- `agentcloudctl inspect --session S`: folded state, including `halted`.
- Hosts are reached by ssh, or, where ssh is refused, by a managed runtime's node-exec command
  that runs an argv on a named node. Whether that command passes stdin is unverified.

### 2.5 Chat bridges

- `agentctl chat run` routes every thread of its channels to one agent in one registry and holds
  an exclusive lease on its state. "Two bridges cannot split one shared channel by thread"
  (`embedded_chat_userguide.md:270-276`).
- **What the Slack provider admits** (`rs/chat-subscription-slack/src/plugin.rs:122-124,
  1043-1066`, at `2a9d8012`): a message is refused as a "non-message subtype" unless it has no
  subtype or one of `thread_broadcast`, `file_share`, `me_message`, so `bot_message` posts are
  dropped; and it is refused as "sender not allowed" unless its user ID is in the allowlist.
  `app_id` and `bot_id` never decide authorship. "The bridge's own posts are written as its app's
  bot user, a different user ID, which is never an allowed sender, so they cannot come back as
  requests." The tail must run as a human member, because Slack does not deliver channel events
  to an app's identity (`embedded_chat_userguide.md:2162-2170`). **So today one team's bridge
  cannot hear another team's bridge at all**, and adding the peer to `allowed_senders` would
  give it the owner's authority.
- **Outbound idempotency.** Slack takes no idempotency key. The outbound helper keeps a ledger
  per operation UUID and, after an uncertain outcome, lists the thread for a matching post from
  the app; if none is found it **posts again** after `--uncertain-repost-after-seconds` (default
  120) (`embedded_chat_userguide.md:2144-2153`). Chunks above 11,000 bytes become consecutive
  posts. A provider-level resend is therefore possible by design.
- A bridge reads reply markers on the agent's output rows, and in a long idle Claude Code pane
  it can also count markers inside a received prompt (`embedded_chat_userguide.md:674-680`).

## 3. Principles

1. **The local agentctl does the TUI work; nothing drives Herdr from afar.** The owner's
   words: "the local agentctl should do the actual TUI manipulation and we shouldn't directly
   start messing with Herdr from afar." No route types into, reads, or drives a remote host's
   Herdr panes, and no route reaches a remote Herdr socket. Every remote route ends at the
   receiving host's own `agent-teams accept`, which enqueues; that host's own drainer
   (agentctl's drain, a bridge, or another local deliverer, section 4.4) does the typing, under
   the single-typist rule. Verification of the coordinator (live record, pinned harness
   process, current boot, real bridges) is done by the receiving host, never inferred by the
   sender from remote reads. Where the coordinator is an agent-runtime session (agentcloud),
   the runtime's own message API is the "local" owner of that session's input, and its own
   `inspect` is the verification (section 10.5).
2. **Exactly once, by construction.** Delivery is made idempotent at the one place every route
   converges, the receiving team's ledger, and every retry rule is written so that a route
   failure can delay a message or strand it for a human, but never deliver it twice (section 9).
3. **Components work alone.** Team messaging must not require agentctl subagent management, a
   chat bridge, ssh, or any company-internal channel; and none of those may require team
   messaging. A route that is not configured, or whose peer does not advertise support, is
   simply not tried (section 4).
4. **No company-specific knowledge in code.** Drivers are generic; everything about a
   particular runtime, node-exec command or chat workspace is in a data file the owner keeps
   (section 10.4).
5. **Coordinators talk to coordinators.** A team's outward voice is its coordinator (or a
   registered service); workers ask their coordinator (section 8.2).
6. **Stale until re-checked.** Owner-level data is a routing hint; authority is on the target
   host, read at delivery time.

## 4. Where team messaging lives: composability

The owner: "Another big requirement of our whole setup is that our tools be à la carte, unlike
Gastown." agentctl already combines Herdr agent management and the chat bridge in one binary;
adding team messaging to it as revision 1 proposed (`agentctl team ...`, `agentctl teams ...`)
makes a fat multi-purpose binary. This section looks only at team messaging. The whole-suite
factoring, including the chat bridge's split from agent management, is
`#498 suite-a-la-carte` ([`2026-10-24-suite-a-la-carte.md`](2026-10-24-suite-a-la-carte.md));
4.7 says where this section agrees with it and where it goes further.

### 4.1 What team messaging actually needs from agentctl

Reading revision 1 for every agentctl dependency gives a short list:

| Needed | Is it agent management? | Where it belongs |
|---|---|---|
| The queue directory format, enqueue with duplicate check, the new lane, tombstones and `not_after` | no: it is the delivery queue | a shared library |
| The pre-paste re-check of tombstones and expiry | no, but it runs inside the drain | the drain (agentctl), via the library |
| Harness identity: `(boot_id, pid, starttime_ticks)`, ancestry walk, pinning | no: it is process identity | the shared library |
| "Who is this registry's coordinator?" | yes, a fact agentctl owns | agentctl writes it; teams read it |
| Record liveness (lifecycle, pane resolves) | yes | agentctl, as `agentctl status --json` |
| Typing into a pane | yes | agentctl only (principle 1) |

Team messaging needs agentctl only at the final hop, and only when the coordinator is an
agentctl agent. Everything else (teams file, routes, outbox, ledger, policy, receipts) has
nothing to do with Herdr or subagents.

### 4.2 Options

- **A. Subcommands of agentctl** (revision 1). One binary to install. But a team-messaging-only
  user installs the whole Herdr agent manager, its help grows another two command families, and
  ssh links, route drivers and private route files land in the tool that types into panes.
- **B. A separate tool, `agent-teams`, sharing a small library with agentctl.** The library
  (working name `agent-queue`, a crate in the Rust workspace) holds the queue format, ledger
  primitives (atomic claim, seen-set) and harness identity. agentctl depends on it for enqueue
  and drain; `agent-teams` depends on it for enqueue (when it delivers into an agentctl queue)
  and for sender proof. Neither binary depends on the other.
- **C. A separate tool that shells out to `agentctl send`.** No shared code, but `agentctl send`
  drains as it sends and so may type (the hazard the helper in 2.3 defers on), and it cannot
  write the new lane or tombstones. It would also need an agentctl binary on every receiving
  host, even one with no agentctl.

Name: `agent-teams` (descriptive, matches `agent-usage`); `teamctl` is shorter but reads as
another controller of agents. The owner decides (question 13).

### 4.3 Dependency graph (option B)

```
                 +-------------------+
                 |  agent-queue lib  |   queue format, lanes, tombstones, not_after,
                 |  (Rust crate)     |   ledger claim, harness identity and ancestry
                 +---------+---------+
                    ^             ^
          links     |             |    links
        +-----------+---+     +---+-----------------+
        |   agentctl    |     |    agent-teams      |  teams.json, routes.d, outbox,
        | agent mgmt +  |     | ledger, accept,     |  receipts, policy, route drivers
        | drain (types) |     | deliverers          |
        +-------+-------+     +----+-----------+----+
                ^                  |           |
   reads queue  |   enqueues into  |           | runs commands described in data
   (drains)     +------------------+           v
                     (agentctl deliverer)   ssh | node-exec | chat helper | ...
        +---------------+
        |  chat bridge  |  (today inside agentctl; may move)  -- optional relay channel:
        +---------------+     pipes an admitted peer post to `agent-teams accept --via relay:R`
```

- agentctl knows no team concepts. It gains only generic queue features in the shared library
  (the `inbox.v2/` lane with a lock-free enqueue, `not_after`, `cancel/` tombstones,
  `cancelled/`, and an optional **external delivery record**: a file path an item may carry,
  which the drain updates before and after the paste and which makes `withdraw` refuse the
  item; 9.2, 9.5), useful to local sends too, and a generic "coordinator designation" it
  already half has (`restore.json`'s `coordinator`).
- The chat bridge knows nothing about teams. It gains one optional, generic feature: a channel
  marked as a relay channel, whose admitted peer posts are piped to a configured command instead
  of being typed (section 10.6).
- Transports are external programs (OpenSSH, a node-exec command, a chat outbound helper),
  invoked from data.
- Version skew between the two binaries is handled where it already must be: the queue lane
  older drainers never list, and `agentctl capabilities` (section 12.4).

### 4.4 Deliverers: what types into the coordinator

`agent-teams accept` never types. After it claims and records a message it hands it to the
team's **deliverer**, chosen in the team's declaration:

| Deliverer | What it does | Wakes an idle agent? | Needs |
|---|---|---|---|
| `agentctl-queue` | enqueues into the coordinator's agentctl queue (`inbox.v2/`); agentctl's drainers type it | yes | agentctl and Herdr on that host |
| `hook-inbox` | writes to a per-team inbox directory; a harness hook (`agent-teams hook`, for example a Claude Code `UserPromptSubmit` or `Stop` hook) claims items by atomic rename and prints them into the agent's context at the next turn boundary | **no**: delivered at the next turn boundary | only the harness's hook support |
| `command` | runs an owner-supplied argv with the rendered message on stdin (their own harness API, a notifier) | depends on the command | the owner's command; its typing is outside this design's guarantees |
| `session` | an agent-runtime session through a data-described command route (10.5) | runtime-dependent | the runtime's CLI |

`hook-inbox` is the pattern Gas Town's `gt nudge` falls back to (a file queue a Claude hook
drains at the next turn boundary; `common/docs/agentctl/RELATED_WORK.md:123-130`). It needs no
terminal server and never types, but a message to an agent idle at its prompt waits for the
next human turn; the receipt says `delivered-at-next-turn` is pending, and the sender sees the
target is hook-only.

### 4.5 A team-messaging-only user

Someone who clones agent-utils, wants their two coordinators on two machines to message each
other, and never wants agentctl subagents:

1. Installs `agent-teams` (one binary; no Herdr, no agentctl, no chat bridge).
2. Writes `~/.config/agent-teams/teams.json` with two teams and one route each (an `ssh` route
   with a dedicated key, section 10.3), and runs `agent-teams ssh enroll` to print the
   restricted `authorized_keys` line for each side.
3. On each host, from inside the coordinator session: `agent-teams init TEAM --coordinator-self
   --deliver hook-inbox`, which pins the session's harness process as the team's coordinator
   (as `agentctl attach-self` pins one) and prints the hook to add to the harness settings.
4. Sends with `agent-teams send other-team --file msg.md` from inside the coordinator; the
   other coordinator sees it at its next turn boundary with its ID and a reply command.

Nothing in that path touches Herdr. If they later adopt agentctl, they change the deliverer to
`agentctl-queue` and messages start waking idle agents; nothing else changes.

### 4.6 Recommendation

Option B: a separate `agent-teams` tool, with the queue format, ledger claim and harness
identity extracted into a shared crate that agentctl also uses; agentctl gains only generic queue
features and no team commands; the chat bridge gains only a generic relay-channel hook;
integration is by data and commands. The rest of this document uses `agent-teams`; if the owner
prefers subcommands, `agent-teams X` maps one-to-one to `agentctl team X` and nothing else in
the design changes. The extraction is the first implementation step (section 17), and it also gives the
Python edition one documented queue format to follow.

### 4.7 Relation to the suite factoring audit

`#498 suite-a-la-carte` recommends (its section 2.4) keeping one `agentctl` command built from
libraries: `agentctl-core` (registry, queue, drainer, target lock, Herdr client, harness TUIs,
subagents, inbox), `agentctl-chat`, and later `agentctl-teams`, with chat and teams as default-on
Cargo features, a CI build of core alone, a check that teams never depend on chat, and an
explicitly versioned registry and queue format.

**Agreed:** team messaging is its own library; it never depends on chat ("no bridge configured"
is normal); the on-disk queue format, versioned, is the contract between binaries; a build
without the feature refuses team commands with exit 2.

**Where this section goes further, for the owner to weigh (question 13):**

- **The library team messaging depends on can be narrower than `agentctl-core`.** It needs the
  queue format, the ledger claim and harness identity (4.1), not the Herdr client, harness TUIs
  or subagents. If those three live in their own small crate (`agent-queue` here; a module of
  core that is also a crate would do), `agentctl-teams` links that and not the rest of core.
- **Herdr is not inherent to team messaging.** The audit's scenario 3.2 ("only team messaging")
  lists agentctl and Herdr on every coordinator host, because delivery there means typing into a
  pane. With deliverers (4.4), Herdr is needed only by the `agentctl-queue` deliverer; a
  `hook-inbox` team needs neither.
- **A binary of its own** follows from those two: a teams-only user installs `agent-teams`
  without the agent manager. That is the audit's option B applied to teams instead of chat, and
  it costs one more binary target on the same library, as the audit notes for the bridge.

Both shapes can coexist: `agentctl-teams` as the library, built into `agentctl` behind the
`teams` feature (`agentctl team ...`) and also as a standalone `agent-teams` binary that links
only the narrow queue crate. The recommendation above stands, with that as an acceptable
compromise.

## 5. Model

```
sender host                                         receiving host (authoritative, fresh)
-----------                                         -------------------------------------
teams.json (sparse hint)                            $XDG_STATE_HOME/agent-teams/teams.d/tools/
  team "tools" entry on host-a                        declaration.json  (team, policy, deliverer,
    contacts: [agentctl-queue coordinator,                               coordinator designation)
               session 0000...]                       ledger/           (seen-set: every ID ever
  host-a routes: [link-a, ssh-a, relay-coord]                            claimed, with its state)
routes.d/*.json (route definitions, may be private)
outbox/<id>.json (envelope, proof, attempts)        agent-teams accept -> claim in ledger ->
     |                                                 deliverer (agentctl queue inbox.v2/,
     +-- route: local | ssh | command | chat relay -->   hook inbox, command)
                (every host route ends at accept)    local drainer types it (agentctl drain)
```

- **Team.** A name (`^[a-z0-9-]{1,32}$`). A team may have **entries** on several hosts (the
  consuming project in 2.3 runs one coordinator per host); `team@host` picks one.
- **Entry.** The coordinator of one team on one host, with ordered **contacts**.
- **Contact.** A destination with its own dedupe authority: `host` (the entry host's
  `agent-teams accept`, whatever its deliverer) or `session` (an agent-runtime session reached
  through a session-mode command route, for coordinators that no host-side agent-teams serves).
  There is no `herdr` and no `chat` destination: nothing reaches a pane except the local
  drainer, and a chat channel is a route to `accept`, not a destination (10.6).
- **Route.** How the sender reaches a contact's host: a name defined in `routes.d`, with a
  built-in driver. Several routes may lead to one host contact; they are interchangeable for
  retries because they converge on one ledger (9.3).
- **Declaration.** `teams.d/<team>/declaration.json` on the receiving host, written by
  `agent-teams init` and owned by agent-teams. It says which deliverer serves the team and how
  the coordinator is designated: by an agentctl registry (the registry's own coordinator
  designation, kept current by agentctl's `rename`, `restore` and `adopt`), or by a pinned
  harness identity (`--coordinator-self`, for hook and command deliverers). Exactly one
  declaration per team per host.

Why two levels: the owner's file changes when a team is created, moved or retired; the
coordinator's identity changes whenever it is revived, renamed or re-adopted, on the host where
that happens. Routing on the first and resolving on the second means a stale owner file costs a
warning, not a misdelivery.

## 6. Data files

### 6.1 Format: JSON, versioned, with explicit comment fields

The owner: "If we use JSON for the other machine-readable agentctl state, we can continue with
JSON here. The main reason to use YAML would be if we want to add some extra metadata in the
comments, but we could always have an explicit comment field in the JSON." agentctl's state is
all JSON (2.1), including the owner-edited `profiles.json` and `chat.json`, so every file here
is JSON:

- **Schema.** Each file starts with `"schema": "agent-teams-<kind>/v<major>"`
  (`agent-teams-teams/v1`, `agent-teams-routes/v1`, `agent-teams-declaration/v1`,
  `agent-teams-message/v1`). A reader refuses an unknown major. Within a major, changes are
  additive optional keys; an unknown key is a warning (`teams check` prints it), and an error
  under `teams check --strict`, which a dotfiles repository's own CI can run to catch typos.
- **Comments.** Any object may carry `"comment"` (a string or an array of strings, for people)
  and `"notes"` (an object, free-form, for people or other tools); both are ignored by the
  reader and **preserved** by every machine rewrite. That is an advantage over YAML comments,
  which a machine rewrite such as `teams discover --apply` would lose.
- **Writes.** Machine writes keep key order, indent by two spaces, end with a newline, are
  atomic (write and rename), and keep the previous file as `<name>.<timestamp>`, so diffs stay
  reviewable.
- **Permissions.** No file holds secrets (keys are referenced by path), but they route messages,
  so each is refused when writable by group or others.

### 6.2 `teams.json`

Default `${XDG_CONFIG_HOME:-~/.config}/agent-teams/teams.json`; `--teams FILE` or
`AGENT_TEAMS_FILE` overrides.

```json
{
  "schema": "agent-teams-teams/v1",
  "comment": "Owner-level routing hints. Edited by people and by `agent-teams discover --apply`.",
  "hosts": {
    "host-a": {
      "routes": ["local"]
    },
    "host-b": {
      "comment": "host-a may ssh here with the agent-teams key; host-b cannot reach host-a.",
      "agent_teams": "~/.local/bin/agent-teams",
      "routes": ["link-b", "relay-coord"],
      "services": {
        "tick-hub": {"unit": "tick-hub.service", "comment": "operational alerts"}
      }
    },
    "host-c": {
      "comment": "ssh is refused here; reached through a private command route from routes.d.",
      "agent_teams": "~/.local/bin/agent-teams",
      "routes": ["node-exec-c"]
    }
  },
  "teams": {
    "tools": {
      "description": "develops the shared tooling",
      "primary": "host-a",
      "entries": [
        {
          "host": "host-a",
          "coordinator": "coordinator",
          "comment": "informational; the host's declaration is authoritative",
          "contacts": [{"kind": "host"}]
        }
      ],
      "policy_hint": {"accepts_from": ["sim", "service:tick-hub"]},
      "reviewed": {"at": "2026-10-24T12:00:00Z", "by": "teams discover --apply"}
    },
    "sim": {
      "primary": "host-b",
      "entries": [
        {
          "host": "host-b",
          "coordinator": "sim-coordinator",
          "contacts": [
            {"kind": "host"},
            {"kind": "session", "route": "cloud-session", "session": "00000000-0000-0000-0000-000000000000",
             "comment": "fallback for when host-b's agent-teams cannot be reached"}
          ]
        },
        {"host": "host-c", "coordinator": "coordinator", "contacts": [{"kind": "host"}]}
      ]
    }
  }
}
```

- `hosts.<h>.routes` is the ordered list of route names to try for `host` contacts on that host
  (section 11). `local` is built in; other names come from `routes.d`.
- `agent_teams` is the absolute or `~/` path of the binary on that host, required for any
  remote route.
- A `session` contact names its own route (a session-mode command route).
- `policy_hint` is advisory; the receiver's declaration decides.
- Freshness: `reviewed` per team is the last human or `discover --apply` confirmation; machine
  observations (last verified, last confirmed delivery, fingerprint seen) live in
  `$XDG_STATE_HOME/agent-teams/observations.json`, never in this file; `teams show` prints ages
  and marks anything unverified for 7 days `stale?`.

### 6.3 `routes.d/*.json`

`${XDG_CONFIG_HOME:-~/.config}/agent-teams/routes.d/`, every `*.json` file read in name order;
a route name defined twice is an error. Splitting routes out of `teams.json` lets a private
file (for a company-internal channel) live in a private dotfiles repository while `teams.json`
and the public code stay generic. The formats are in section 10.

### 6.4 `declaration.json` (receiving host)

```json
{
  "schema": "agent-teams-declaration/v1",
  "team": "tools",
  "deliverer": {"kind": "agentctl-queue", "registry": "/home/u/work/dev-tools/.agentctl"},
  "coordinator": {"designation": "registry",
                  "comment": "the registry's own coordinator designation; agentctl keeps it current"},
  "accepts_from": [
    {"sender": "sim", "kinds": ["info", "request", "hold", "resume", "reply"]},
    {"sender": "service:tick-hub", "kinds": ["info"], "authority": "peer"}
  ],
  "may_send_to": ["sim"],
  "limits": {"max_bytes": 8192, "per_sender_per_hour": 30, "max_ttl_seconds": 604800},
  "default_ttl_seconds": 7200,
  "relay_channels": [{"route": "relay-coord", "peers": {"U0PEER": "sim"},
                      "comment": "accept envelopes relayed by this host's bridge; each peer bot user speaks for one team"}],
  "updated_at": "2026-10-24T12:00:00Z",
  "updated_by": "agent-teams init"
}
```

For a hook or command deliverer, `coordinator` is `{"designation": "pinned", "harness_identity":
{...}}`, refreshed by re-running `init --coordinator-self` from the new session. A sender in
`accepts_from` is a team name (meaning that team's coordinator) or `service:NAME`; there is no
`team/agent` form, because only coordinators send (8.2).

## 7. Discovery and checking

```
agent-teams discover [--host H]... [--apply] [--json]
agent-teams check [TEAM[@HOST]]... [--strict] [--json]
agent-teams show [TEAM] [--json]
```

`discover` runs, on each host named and through that host's routes, the read-only `agent-teams
probe --json`. The probe runs locally on the target and reports what that host's own tools say;
it does not read panes from afar (principle 1):

1. **Declarations** in `teams.d/`, and for agentctl deliverers the registry's coordinator
   designation, record lifecycle and harness identity (from `agentctl status --json`), and
   whether the record's boot ID is the host's current one.
2. **Unregistered coordinators.** `agentctl probe`'s own list of Herdr panes running a harness
   or `agentterm` that no registry names, with the `agentctl adopt` or `attach-self` command
   that would register them (I1). Absent when agentctl is not installed.
3. **Sessions.** For each session the host can see, its runtime `inspect` state.
4. **Chat bridges.** Every running bridge (from its main process's command line and its state's
   configuration: agent, channels, `reply_alias_range`, relay channels), and each expected unit's
   load, enable and active state. Two running bridges on one pane without disjoint reply ranges
   is a **shared-pane-bridge** finding (I2); an expected unit that is missing, disabled and
   inactive, or whose process names another registry or agent, is a **stale-bridge** finding
   (I4).
5. **Routes, from the sender's side.** For each route: whether it reached the probe, the link
   state if it has one (10.3), whether stdin arrived (the probe echoes a short known string),
   and the peer's protocol version.

`discover` merges the probes into a proposed `teams.json`, prints a unified diff and the
findings; `--apply` writes it (6.1) and stamps `reviewed`. It never runs `adopt`, `init`,
`ssh enroll` or any `systemctl` verb; it prints them. `check` is `discover` without the
proposal: every contact and route verified, as a table; exit 0 when every entry has at least one
verified contact with at least one working route, 1 otherwise. It is cheap enough for a timer.

## 8. Messaging

### 8.1 Command and envelope

```
agent-teams send TEAM[@HOST] (--file FILE | --text TEXT)
    [--key KEY] [--topic SLUG] [--supersedes KEY]... [--ttl DURATION | --expires TIME]
    [--kind info|request|hold|resume] [--private] [--from-service NAME] [--wait SECONDS] [--json]
```

`send` builds an envelope, proves the sender (8.2), writes the envelope to the outbox, and then
runs the route walk (11). The envelope:

```json
{
  "schema": "agent-teams-message/v1",
  "id": "tm-sim-6f1d0c2a9b7e4d31",
  "key": "ci-hold-2026-10-24",
  "body_sha256": "…",
  "from": {"team": "sim", "role": "coordinator", "host": "host-b", "seq": 412,
           "proof": "harness-ancestry"},
  "to": {"team": "tools", "host": "host-a"},
  "kind": "hold",
  "topic": "ci-hold",
  "supersedes": [],
  "private": false,
  "created_at": "2026-10-24T13:00:00.250Z",
  "not_after": "2026-10-24T15:00:00Z",
  "reply_to": {"team": "sim", "host": "host-b", "in_reply_to": null},
  "body": "Please hold landing to main until the mirror is repaired."
}
```

- **ID.** `tm-<sender>-` and 16 hex digits of SHA-256 over (sender kind, sender name, sending
  host, target as addressed (`TEAM` or `TEAM@HOST`), key), where `<sender>` is the team name or
  `svc.<name>` for a service, so the ID passes agentctl's message-ID rule (letters, digits, `.`,
  `_`, `-`; `agent.rs:3127-3139`) and a team and a service never share a namespace. The same key
  sent to `proj@host-b` and to `proj@host-c`, or by the coordinators of one team on two hosts,
  gives different IDs: they are different messages. The ID is fixed when the outbox entry is
  written and never changed by any retry, route or contact (9.1). Without `--key` the key is
  derived from (kind, topic, body) and printed, so re-running an identical command is a retry;
  sending the same text again on purpose needs a new `--key`. `from.seq` is a per-host counter,
  used only for topic order (12.1).
- **Same key, same body** returns the existing message's state from the receiver's ledger.
  **Same key, different body** is refused as a conflict; to change a message, send a new key
  that supersedes the old one.
- **`--private`** keeps the body off routes that expose it to third parties (a chat channel;
  10.6).
- **What is typed.** The receiver renders the envelope on its own host:

  ```
  [team message tm-sim-6f1d0c2a9b7e4d31 from sim (coordinator, proved on host-b, via link-b) to tools; kind hold; topic ci-hold; expires 15:00Z]
  Please hold landing to main until the mirror is repaired.
  [reply: agent-teams reply tm-sim-6f1d0c2a9b7e4d31 --file FILE]
  ```

  The header is generated by the receiver from the verified envelope, including how the sender
  was proved and which route carried it. Body lines that begin with `[team message` or `[reply:`,
  and lines that look like a chat bridge's reply markers, are indented by two spaces, so a body
  can neither forge a header nor be posted by a bridge reading a long idle pane (2.5). The typed
  text never starts with `/`.

### 8.2 Who may send: coordinators and registered services only

The owner: "I don't think workers should message other teams. Only coordinators." This resolves
revision 1's question 8.

- **Coordinator** (`proof: harness-ancestry`). agent-teams walks its own process ancestry for a
  PID whose `(boot_id, pid, starttime_ticks)` equals the harness identity of **the team's
  designated coordinator** on this host: the registry's coordinator record for an agentctl
  deliverer, or the pinned identity in the declaration. Matching any other record in the
  registry is not enough; a worker of the same team is refused with `refused:
  sender-not-coordinator` and the hint "ask your coordinator". In-process helpers of the
  coordinator (its own sub-agents, background jobs it started) share its ancestry and count as
  the coordinator, which is answerable for them. Processes it starts in other panes (for example
  through `herdr-run`) do not; that is why proof is taken at outbox write (below).
- **Registered service** (`proof: service-unit`). Kept, explicitly, because operational alerts
  (2.3) are the first user. `--from-service NAME` is accepted only when NAME is registered for
  this host in `teams.json` with a systemd unit and `/proc/self/cgroup` shows the caller inside
  that unit. It is refused outright when the ancestry walk finds any agent harness, so a worker
  refused as itself cannot retry as a service. A service sends as `service:NAME`, never as a
  team, and is labelled as a service.
- **Workers: never.** Not as themselves, not as their team. A worker that needs another team asks
  its coordinator. Workers are also never recipients: a team message is addressed to a team and
  delivered to its coordinator; `reply_to` names a team, not an agent.
- **agentcloud sessions.** A session coordinator is proved by its runtime (`send-message
  --session <self>` is attributed by the runtime itself) only on a session-mode route that
  declares `attribution: transport` (10.5); `$AGENTCLOUD_SESSION_ID` alone is an assertion. A
  session coordinator that wants host routes registers itself with `agentctl attach-self` or
  `agent-teams init --coordinator-self`.
- **The owner: never.** A process that cannot be proved to be a coordinator or a registered
  service is refused, and team messages never use a runtime's operator mode unless the
  receiving team opts a specific service into it (question 6).

**Proof is taken once, at outbox write, in the sender's own process tree**, and carried by
whatever process runs the route (`agent-teams pump` in a `herdr-run` pane, a timer). For the
`local` route the receiver re-derives the proof from the outbox entry's recorded caller and
refuses a mismatch. For remote routes it records what the route itself authenticates (10.2) and
renders the proof as "proved on host-b", an assertion by that host's agent-teams.

**Isolation, and the honest limit.** The outbox is `$XDG_STATE_HOME/agent-teams/outbox/`.
`send` refuses workers, and `pump` re-checks each entry before every attempt: the recorded
prover must be the team's designated coordinator (or a registered service) as of the entry's
creation, according to the record's history, or the entry is quarantined. That stops a worker
that uses the CLI, and a stale entry left by a former coordinator. It does not stop a worker
that writes a forged entry file directly: under the same user ID, with slots isolated only by
cgroup (no write isolation), the outbox is as writable to a worker as the queue files are, and
so the rule is enforced against confused agents, not hostile ones (13). Slot boxes with write
isolation (user-namespace or root) cannot reach the outbox at all. Each supported coordinator
mode gets a test proving the sender from inside it, and a test that a worker's `send` is
refused.

### 8.3 Verification on the receiving host, at delivery time

Every route ends at `agent-teams accept --protocol 1` on the target host, so verification is the
same whichever route carried the envelope; only the receiver's agentctl and the receiver's own
reads take part:

| Check | `host` contact |
|---|---|
| Protocol | `accept --protocol 1` runs; an older binary is a definite `remote-too-old` refusal |
| Team | exactly one declaration for the team on this host |
| Coordinator is live | agentctl deliverer: the designated record is `running`, its boot ID is the current one, the pinned harness process is alive with the same start ticks, the pane resolves (all from `agentctl status --json`, run locally). Pinned designation: the harness process is alive with the same identity |
| Drainer exists | agentctl deliverer: a running bridge or watcher on this queue whose binary understands the lane, or the host's `agent-teams drain` timer; otherwise the receipt says `queued-no-drainer`. Hook deliverer: `hook-only` |
| Bridge sanity | stale-bridge and shared-pane-bridge findings (7) travel in the receipt; the contact is still used if the coordinator verifies (I4: never a silent fallback) |
| Policy | sender and kind in `accepts_from`, sender in its own `may_send_to` (checked by both sides), size and rate limits, `not_after` within `max_ttl_seconds` |
| Freshness | `not_after` not passed; `created_at` not older than the ledger's retention horizon (9.2) |

Verification for `session` contacts is the runtime's own `inspect`, run by the sender through
the session route before and after delivery (10.5). Section 11.4 tabulates what each route adds.

### 8.4 Outbox, ledger, receipts and states

- **Sender:** `outbox/<id>.json` (envelope, proof, and every attempt: route, contact, phase,
  classification, timestamps), `team-log.jsonl`, and observations.
- **Receiver:** `teams.d/<team>/ledger/<id>.json`: body SHA-256, sender, route, deliverer
  target, state, timestamps, and the newest `(created_at, host, seq)` seen per sender and topic.
  The ledger is the dedupe record (9.2).

| State | Meaning | Retried? |
|---|---|---|
| `claimed` | the ledger entry exists, naming its hand-over target; the hand-over is not confirmed yet (recovered by the next `accept` or `drain`, 9.2) | no new claim; recovery finishes it or records `possibly_submitted` |
| `accepted` | handed to the deliverer (in the agentctl queue, or the hook inbox), not yet typed or read | the deliverer delivers it |
| `typing` / `presented` | the drain is about to paste it / the hook has printed it; not yet confirmed | never; becomes `delivered` or `possibly_submitted` |
| `delivered` | agentctl: the drain confirmed the paste; hook: a later hook run in the same harness confirmed it | no |
| `expired` | `not_after` passed before delivery | no; sender told |
| `superseded` | a newer message on its topic, or one naming its key, cancelled it first | no; sender told which |
| `retracted` | `agent-teams retract` cancelled it first | no |
| `fenced` | the sender asked the receiver to bar this ID before it arrived (9.4) | never accepted here |
| `possibly_submitted` | stranded in `inflight/` or uncertain in `failed/` | **never**; a human resolves it |
| `lost` | in the ledger as accepted but in no queue phase | **never**; reported until resolved |
| `refused` | not handed over: policy, verification, conflict | under a new key only |
| `deferred` | sender-side only: no contact has given a definite answer yet | `pump` resolves, then retries (11) |
| `ambiguous` | sender-side only: a hand-over may have happened; awaiting a receipt | never resent blind (9.3) |
| `ambiguous-expired` | sender-side only: still ambiguous when `not_after` passed | **never**; a human resolves it (question 19) |
| `awaiting-pull` | sender-side only: the only route to the contact is the receiver pulling (10.3), or a reply waits for the original sender to collect it (8.5) | the puller takes it |
| `journaled` / `journaled-halted` | session contact: in the session's journal; `-halted` when the runtime shows it will not run | no |

Receipts read the ledger and list queue directories without taking the delivery lock, so
`agent-teams receipt ID` answers at once while a drainer waits on a busy pane. Receipts are
**pulled** by the sender through a route; nothing on the target has to reach back, except where
the only route is the receiver pulling (10.3), in which case the receiver pushes receipts with
its acknowledgement.

**Retention.** Terminal ledger and outbox entries are kept 30 days (and at least as long as 9.2
requires); `possibly_submitted` and `lost` until resolved. The queue's `processed/` is outside
this design (question 9).

### 8.5 Replies

`agent-teams reply ID --file FILE`, run by the receiving team's coordinator (proved as in 8.2),
builds an envelope to `reply_to` with `in_reply_to: ID`. Its key defaults to `reply:<ID>`, so
re-running the same reply is a retry and a reply to a duplicate sighting of the same message is
the same reply (9.6). A second, different reply needs `--key`. It travels over the replying
host's routes to the sender's team; where the replying host has none, it waits in the outbox as
`awaiting-pull` and the original sender's `pump` collects it (`agent-teams pull --for sim`). A
reply is delivered to the sender team's coordinator, never to a worker.

## 9. Exactly-once delivery

The owner: "One of the most important things is that these messages don't get duplicated,
especially if there are multiple routes to attempt delivery or to retry." Two of the incidents
are duplicates: I5 (a retry after a slow, unclear send posted a message three times) and I2 (a
bridge reported a reply as unsent, so the agent sent it again). Both are the same mistake: an
unclear outcome answered by sending again, to a receiver that could not recognise the second
copy. Multiple routes multiply the chances of making it.

### 9.1 What "exactly once" means here

Strictly, no system over unreliable links can promise that a message is delivered exactly once
*and* always delivered. This design promises:

- **At most one enqueue per ID per team**, across all routes, retries, restarts and hosts of
  that team, and **at most one paste per ID** into any pane;
- **at least once while things work**: if some route to a verified contact stays up long enough
  before `not_after`, the message is delivered;
- **never silently in between**: when neither can be shown (a crash between hand-over and
  typing), the message ends `possibly_submitted` or `lost`, visible to the sender and the owner,
  and is never retried automatically. That is agentctl's existing rule, kept.

### 9.2 One ID end to end, and a durable seen-set at the receiver

- **One ID, assigned at send.** The ID is fixed when the outbox entry is written (8.1) and is
  carried in the envelope on every route. Where a route has a native idempotency key, the route
  passes a key derived from the ID (the ID itself, or `<id>/<route>` where a provider scopes keys
  per sender): `--idempotency-key` for a runtime CLI, the operation UUID for a chat outbound
  helper (2.5). Native keys are a second line of defence; the receiver's ledger is the first,
  because several routes have none (ssh, a node-exec command) and native keys do not span routes.
- **The seen-set is the team's ledger** on the receiving host, one file per ID. `accept`
  **claims** an ID by creating `ledger/<id>.json` with exclusive create (`O_EXCL`, under the
  ledger's own short lock, never the queue's delivery lock), recording `claimed` with its own
  process identity, *before* it hands anything to the deliverer. Only the claimer hands over. A
  second arrival of the same ID, by any route, at any time, reads the existing entry and returns
  its state: same body, the current state; different body, `conflict`. Two arrivals racing (one
  over ssh, one relayed by the bridge) cannot both claim.
- **The claim names its hand-over target.** Before handing over, `accept` records in the claim
  the exact target: deliverer kind and, for `agentctl-queue`, the queue directory and the
  record's `token`. The queue item carries the ledger entry's path, and the shared library's
  drain updates that entry itself (`typing` just before the paste, then `delivered` or
  `possibly_submitted`), so the ledger never depends on the queue still existing to know an
  item was typed. agentctl learns no team concept from this: an item may carry an external
  delivery record, and the library keeps it current (4.3).
- **Crash between claim and hand-over.** Recovery runs only in `accept` and `drain` (never in
  `receipt`, which stays a pure read). It sees `claimed` with a dead claimer and then:
  - `agentctl-queue`: looks for the ID in the recorded queue, in every phase including
    `inbox.v2/`, `cancel/` and `cancelled/`, **and** under `REGISTRY/archive/` if the record was
    stopped since. Found: records the state it shows. Proven absent from the recorded queue,
    which still exists with the same record token: re-runs the hand-over (the enqueue refuses
    an existing ID; `cli.rs:645`). Anything else (the queue archived and pruned, the token
    changed, a phase unreadable): `possibly_submitted`, never a second hand-over.
  - `hook-inbox`: the inbox is keyed by ID, so re-writing is idempotent; same proof rule.
  - `command` and `session`: these hand-overs are not idempotent (an owner command, a
    networked runtime call), so recovery records `possibly_submitted`, unless the deliverer
    declares a status query (10.5) that answers within its idempotency window.
  - `withdraw` refuses items that carry an external delivery record, so an item cannot leave
    the queue except by delivery, cancellation or quarantine.
- **Enqueue does not wait for a drain.** Today `enqueue` takes the queue's delivery lock
  (`agent.rs:797-830`), which a drain holds through readiness waits of up to 900 s (2.1); an
  `accept` blocked there inside an ssh session would time out as ambiguous and pin the message,
  the I3 window again. The `inbox.v2/` lane therefore takes a lock-free enqueue: write a private
  temporary file, then `link()` it to `inbox.v2/<id>.json`, which fails if the name exists;
  duplicates across phases are refused by the ledger claim that precedes it, and the drain moves
  items onward with a no-replace rename. The delivery lock still serializes drains.
- **Durable, not in memory.** The fix for I5 remembers keys for 24 h in a server's memory, which
  suits a page retrying within seconds. Here restarts of either side are among the failures, and
  retries come from timers hours later, so the seen-set is on disk and survives restarts,
  `agentctl stop`, archive, rename, and a change of the coordinator's registry.
- **Retention longer than any retry window, by construction.** Every message has a `not_after`
  no later than `created_at + max_ttl_seconds` (default 7 days); `accept` refuses an expired
  envelope and one whose `created_at` is older than the retention horizon; each ledger entry is
  kept until 30 days past **its own stored `not_after`** (not the current `max_ttl_seconds`,
  which the owner may change). So by the time an entry may be pruned, every copy of that
  envelope still in flight anywhere would be refused as expired: pruning can never reopen an
  ID. `--ttl none` is therefore not offered (question 4).
- **Addressed to one host.** `accept` refuses an envelope whose `to.host` is not its own host,
  before claiming, so a relay post or a pull meant for `sim@host-b` cannot be claimed by
  `sim@host-c`.
- **One team, several hosts.** A team with entries on two hosts has two ledgers. The ID is
  addressed to one entry (`to.host`); moving it to another entry is a change of contact and
  follows 9.4, so two ledgers never both accept one ID.

### 9.3 Retrying on the same contact: any route, after asking

All `host` routes to one entry converge on that entry's ledger, so a resend of the same ID to
the same contact, over the same route or another one, cannot enqueue twice. The rule for an
ambiguous outcome is still **ask before resending**:

1. An attempt ends **definite** (a refusal or `accepted` answered by `accept`, or a failure the
   route proves happened before the envelope could reach the host; 10.2) or **ambiguous**
   (anything else: a timeout, a connection dropped after the remote command started, a chat
   post with no receipt yet, an unreadable answer).
2. After an ambiguous attempt the sender records `ambiguous` and asks the receiver: `receipt ID`
   over the first route in order that answers. A known state ends the matter.
3. Only when the receipt says the ID is **unknown** does the sender resend, still with the same
   ID, to the same contact (over any route). Even if the earlier attempt lands later, the ledger
   absorbs it.
4. If no route can answer, the message stays `ambiguous` and `pump` asks again later. It is
   never resent blind.

The ask costs one round trip and keeps logs honest ("resent after the receiver reported the ID
unknown at 13:04:10Z" rather than "retried"). It also covers the rare case where the ledger is
unreachable but the queue is not.

### 9.4 Trying a second contact: only after proof, never after ambiguity

A different contact has a different dedupe authority (another host's ledger, or a runtime's
journal), so trying it after an ambiguous attempt could deliver twice: the I2 pattern by another
road. The rule:

- **A contact is released** (the walk may move on) only when every attempt to it is definitely
  undelivered: every attempt was refused by `accept`, or failed in a way the route proves
  preceded hand-over, **or** the receiver has **fenced** the ID. One ambiguous attempt anywhere
  in the contact's history means a fence is required, whatever later attempts answered.
- **Every refusal is a fence.** `accept` writes its refusal (policy, verification such as "the
  coordinator is not live", conflict, wrong host) as a durable ledger entry for that ID under
  the ledger lock, so the same ID arriving later by any route (a relay post read hours later, a
  pull) is refused too. A refusal that leaves no ledger entry (`remote-too-old`: the host has
  no agent-teams that understands the protocol) releases the contact only if no earlier attempt
  to it was ambiguous.
- **Successor IDs, not reopened IDs.** A refused ID is never accepted later on that contact. When
  the cause was transient (the coordinator was restarting) and no other contact delivers,
  `pump --retry-refused` may send a **successor**: the same body under key `<key>~2` (a new ID),
  only when every earlier ID of the message is refused or fenced at every contact it was offered
  to. Each successor is a new message that its predecessors provably did not deliver.
- **A fence** is `agent-teams fence ID` on the receiving host: under the ledger lock, if the ID
  is unknown it writes a `fenced` entry, so any later arrival of that ID, by any route, is
  refused; if the ID is known it returns its state instead. A fence is how "the receipt says
  unknown" becomes proof that it will stay unknown: without it, an attempt still in flight (a
  slow ssh command, a chat post the receiving bridge has not read yet, a pull not yet taken)
  could land after the sender moved on.
- **A contact is pinned** once any attempt to it was ambiguous and it cannot be fenced (no route
  to its host answers), or once it answered `accepted`. A pinned message is never tried on
  another contact; `pump` keeps asking the pinned contact until it answers or `not_after`
  passes, and then reports `ambiguous-expired` for a human. This keeps revision 1's rule ("pin
  once handed over", as `inbox deliver` does) and refines it: a fence can unpin.
- **Session contacts** have no fence. A session-mode route may declare a `status` operation
  that reports whether the runtime journaled a key (10.5). "Unknown" counts as proof only
  **within** the route's declared idempotency window, after the `deliver` process has exited and
  the route's declared `settle_seconds` have passed (a timed-out call may still be processed on
  the server, which is I5's shape). Past the window the runtime has forgotten the key and
  "unknown" proves nothing, so the contact stays pinned; without a `status` operation it is
  pinned by any ambiguous attempt.

### 9.5 Never two pastes for one ID

The drain (agentctl, through the shared library) already moves an item to `inflight/` by atomic
rename before typing, so two drainers cannot take one item, and an uncertain paste becomes
`possibly_submitted`, never retyped. For items that carry an external delivery record, the
drain also checks, just before the paste and together with the tombstone and expiry re-check
(12.2), that the record is still `claimed` or `accepted` for **this** queue, and writes `typing`
to it before pasting. A record already at `typing`, `delivered` or `possibly_submitted`, or
naming another queue, means another copy exists; the item is moved to `cancelled/` with reason
`duplicate` and is not typed.

The hook deliverer claims an item by rename and records `presented` (an uncertain state) before
printing it. The next run of the hook in the same harness process (same harness identity) is
evidence that the harness took the earlier output, and upgrades `presented` to `delivered`. A
harness that exits or crashes first leaves the item `possibly_submitted`, never presented
again.

### 9.6 What the coordinator sees if a duplicate slips through

Duplicates should not reach a coordinator, but two paths remain where a second sighting is
possible: a message delivered through a `session` contact and later through a host contact
after a human resolved an `ambiguous-expired` message, and a body a human pasted by hand. For
those:

- The header always shows the ID, and a host-side delivery of an ID the ledger already records
  as delivered (it can only come from a human's explicit `agent-teams redeliver ID`) is rendered
  `[team message tm-… — ALREADY DELIVERED 13:02Z via link-b; shown again at the owner's request]`.
- The footer tells the coordinator that an ID it has already handled needs no action, and
  `agent-teams seen ID` answers "delivered at 13:02Z" from the local ledger.
- A reply defaults to key `reply:<ID>`, so answering both sightings sends one reply, deduped by
  the sender's ledger.
- No footer ever asks the agent to resend anything: the lesson of I2 is that "not sent" said to
  an agent produces a second send.

### 9.7 Tests the implementation must have

Exactly-once is a property of every interleaving, so example tests are not enough. Phase 3 must
include a **model-based harness** (a deterministic simulation with a seeded schedule, replayable
from the seed of any failure, in the style of property-based stateful testing):

- **Model:** two to four hosts, teams with one or two entries, contacts of both kinds, two to
  four routes per host (local, ssh-like, command-like, relay-like), real ledger and queue code
  on a temporary filesystem, a fake pane that counts pastes per ID, and a fake runtime journal
  with declared idempotency semantics.
- **Faults, chosen at random per step:** a route failing before the remote command starts;
  after it starts but before the answer; with the answer lost; with the command still running
  after the sender gave up; delayed delivery (a relay post read minutes later, a pull taken
  late); duplicate delivery by the route itself (a chat helper's repost, a stream replay);
  partitions that cut one host from some or all routes for a while; receiver crashes (kill at
  every step of accept: before claim, after claim, after hand-over, before recording `accepted`),
  including a crash followed by the drain typing the item and the coordinator being stopped
  (its queue archived) before recovery runs;
  drainer crashes before and after the paste; sender crashes between the attempt record and the
  route call; clock skew between hosts; ledger pruning at the retention horizon; a coordinator
  rename or registry move mid-flight; a relay post or pull for one entry seen by the team's
  other entry host; a refusal followed by a late copy of the same ID by another route; a session
  `status` query racing a still-running `deliver`; a hook whose harness exits before the next
  turn.
- **Invariants checked after every step:** at most one enqueue per ID per team across all
  ledgers and queues; at most one paste per ID in all fake panes; no paste after `not_after`;
  no attempt on a second contact while the first is pinned; no resend without a preceding
  receipt that said unknown; every message eventually terminal, `ambiguous-expired`,
  `possibly_submitted` or `lost`, each visible in a receipt; and, when routes are healthy for
  long enough before `not_after`, `delivered`.
- **Real-process cases** for the same faults at the seams a simulation cannot see: `kill -9`
  of `accept` and `drain` at named failpoints, an ssh connection cut with the remote command
  running, and an old drainer binary beside a new one (12.4), in both editions where the queue
  is concerned.
- **Regression cases named for the incidents:** a slow multi-part relay post retried by the
  sender (I5), and two bridges sharing a pane with the coordinator replying (I2).

## 10. Routes: a pluggable layer described in data

### 10.1 Drivers in code, routes in data

The code ships a small fixed set of **drivers**; every **route** is a named instance in
`routes.d/*.json` (6.3) that picks a driver and fills in everything else. Nothing about a
particular host, key, runtime or chat workspace is compiled in.

| Driver | Reaches | Envelope travels | Built-in route |
|---|---|---|---|
| `local` | `agent-teams accept` on this host, as a child process | stdin | `local` |
| `ssh` | `agent-teams serve` on a remote host, through OpenSSH, optionally over a managed link | stdin, after a ready banner | none |
| `command` | anything an argv template can run: in `host` mode, `agent-teams serve` on a remote host (a node-exec command); in `session` mode, an agent-runtime session; in `relay` mode, a post to a chat channel | stdin or one argument, as declared | none |

A route file:

```json
{
  "schema": "agent-teams-routes/v1",
  "comment": "Routes from this host. Private routes go in a separate file that is not published.",
  "routes": {
    "link-b": {"driver": "ssh", "target": "host-b.example.net", "...": "section 10.3"},
    "node-exec-c": {"driver": "command", "mode": "host", "...": "section 10.4"},
    "cloud-session": {"driver": "command", "mode": "session", "...": "section 10.5"},
    "relay-coord": {"driver": "command", "mode": "relay", "...": "section 10.6"}
  }
}
```

### 10.2 The contract every host route meets

A `host` route is only a way to run `agent-teams serve VERB` on the target host with an input
and read its output. The verbs are fixed: `accept`, `receipt`, `fence`, `probe`, `ping`, `pull`,
`ack`. Every attempt is classified by the driver into one of:

- `answered` (the remote agent-teams spoke the protocol; the JSON answer decides: `accepted`,
  a known state, `refused`, `fenced`);
- `not-delivered` (**definite**: the route proves the envelope cannot have reached the remote
  `accept`);
- `ambiguous` (everything else).

How a driver proves `not-delivered`:

- **Interactive stdin** (`local`, `ssh`): `serve` prints a ready banner (`agent-teams-serve/1
  ready <host> <boot_id>`) before it reads anything, and the driver writes the envelope only
  after it has read the banner. No banner means the envelope was never sent: definite. Once the
  envelope has been written, a failure is ambiguous. `serve` acts only on a complete,
  length-prefixed envelope whose SHA-256 matches, so a cut-off write does nothing.
- **Argument or non-interactive stdin** (`command` routes that cannot hold a conversation): the
  envelope is in the argv or the first write, so any execution may accept it. Only failures the
  route's data declares as happening before the command runs (10.4's `pre_exec_failure`) are
  definite; everything else is ambiguous.

Each attempt record keeps: route, contact, verb, phase reached (`connect`, `banner`, `sent`,
`answered`), classification, the remote's `boot_id` if seen, duration, exit status, and a
bounded, redacted stderr excerpt.

### 10.3 ssh, without assuming ambient keys

The owner is "concerned about passwordless ssh and the ability to access the other servers".
Revision 1, like the helper in 2.3, used whatever ssh access the account happened to have, which
grants a full shell on the other host to anything that can run ssh. The ssh driver assumes
nothing ambient:

- **A dedicated key per host pair, restricted to the protocol.** `agent-teams ssh enroll --peer
  host-b` creates a key under `~/.config/agent-teams/ssh/` (mode 0600) and prints the line for
  host-b's `authorized_keys`:

  ```
  command="/home/u/.local/bin/agent-teams serve --stdio --peer host-a",restrict ssh-ed25519 AAAA… agent-teams host-a->host-b
  ```

  `restrict` disables port, agent and X11 forwarding, the pty and `~/.ssh/rc`; the forced command
  means the key can run only `serve`, which reads the verb from `SSH_ORIGINAL_COMMAND`, accepts
  only the fixed verbs, and records `--peer host-a` as the authenticated origin of everything it
  receives. A `from="..."` option may be added. agent-teams never edits `authorized_keys`; the
  owner installs the line.
- **No user ssh configuration by default.** The driver runs OpenSSH with `-F none` and explicit
  options: `BatchMode=yes` (never prompt), `IdentitiesOnly=yes` with the route's key,
  `IdentityAgent=none`, `StrictHostKeyChecking=yes` with a known-hosts file of its own (filled at
  enrollment from a fingerprint the owner confirms), `ForwardAgent=no`, `ClearAllForwardings=yes`,
  `ConnectTimeout`, `ServerAliveInterval` and `ServerAliveCountMax`. A route may opt into the
  user's own configuration and agent (`"use_user_ssh_config": true`, for example to inherit a jump
  host), which is labelled in `teams show`.
- **Attribution gets better, not worse.** Because the key is bound to a peer, the receiver knows
  which host's agent-teams handed it the envelope: "proved on host-b, via the host-b key".
- **A host key mismatch** disables that route (never `accept-new` after enrollment) and is
  reported as a finding on every send until the owner re-enrolls; other routes to the host may
  still be used, since each authenticates independently.

**The managed link (optional).** A persistent connection avoids paying ssh setup on every
receipt query and gives a health signal:

```json
"link-b": {
  "driver": "ssh",
  "target": "host-b.example.net",
  "user": "u",
  "identity": "~/.config/agent-teams/ssh/host-a-to-host-b",
  "known_hosts": "~/.config/agent-teams/ssh/known_hosts",
  "link": {"mode": "persistent", "ping_seconds": 30, "down_after_failures": 3,
           "backoff_seconds": [5, 30, 120, 600]},
  "direction": "push",
  "capabilities": {"max_envelope_bytes": 65536}
}
```

- `link.mode`: `none` (a fresh connection per operation), `on-demand` (an OpenSSH ControlMaster
  opened on first use with `ControlPersist` for an idle period, so nothing lingers), or
  `persistent` (kept up by `agent-teams link run ROUTE`, an autossh-style supervisor: it runs the
  ControlMaster in the foreground, pings with `serve ping` through it, and reconnects with
  backoff). The control socket is `$XDG_RUNTIME_DIR/agent-teams/links/<route>.sock`; every
  operation is a new session channel on it, each still subject to the forced command.
- **Lifecycle.** `persistent` links are run by a user unit the owner installs; agent-teams
  prints a unit file or `systemd-run` line and never runs `systemctl` itself. `link up`, `link
  down` and `link status` act on the socket; stopping the supervisor closes the master.
- **Health.** The supervisor writes `$XDG_STATE_HOME/agent-teams/links/<route>.json`: `state`
  (`up`, `degraded`, `down`), `since`, `last_ok`, round-trip time, consecutive failures, last
  error, and the peer's `boot_id` (a change means host-b rebooted). `teams check` and `send`
  read it.
- **When the link is down.** A link known down (state fresher than twice the ping interval) is
  skipped without an attempt, which is definite; the walk goes on to the next route. With no
  usable route the message is `deferred` and `pump` retries. A link that drops mid-attempt
  makes that attempt ambiguous, resolved as in 9.3.

**One-way reachability: pull, not a reverse tunnel.** Where host-b can reach host-a but not the
reverse, host-b runs the link to host-a with `"direction": "pull"`, and host-a's own route list
for host-b names a route of driver `ssh` with `"direction": "await-pull"`, so pull is a step of
the walk like any other route (11.1):

- When the walk reaches that route, the outbox entry becomes `awaiting-pull` **for that contact
  only**. host-b's supervisor keeps a long poll open, `serve pull --for host-b --wait 300`
  (filtered by target host, and so by entry, never by team alone); it returns as soon as
  host-a's outbox holds an entry awaiting host-b.
- `serve pull` offers an entry only under that entry's outbox lock and only while the walk is at
  host-b's contact, and records the offer before returning it. host-b runs `accept` locally and
  returns the result with `serve ack ID STATE`. An offer without an ack is ambiguous; host-a
  offers the same ID again, and host-b's ledger absorbs it.
- An entry never offered may leave the contact: after the route's `wait_seconds`, `pump` closes
  the offer under the entry lock (a later `serve pull` no longer sees it) and the walk moves on,
  which is definite because nothing was handed over. Once offered, the message is pinned to
  host-b (host-a cannot reach host-b to fence; 9.4). This needs only an outbound
connection from host-b and the same restricted key. A reverse port forward (`ssh -R` of a
socket, with a listener on host-b) would also work, but adds a listening service and a
`permitlisten` grant for no gain; it is not proposed.

**Hosts where ssh is refused** get no ssh route: a `command` route (10.4), a chat relay (10.6),
pull from the other side if that host can ssh out, or a session contact. With none, `teams
check` reports `no-route` for the entry.

### 10.4 The command transport, and company-internal channels as data

The owner: "Agentcloud is another interesting route between servers, but it's internal to one
company, so we would need a very flexible description of it in a data file to keep it outside
the code." The `command` driver is that description's interpreter. It knows argv templates,
exit-code tables, JSON pointers and capability flags; it knows nothing about any runtime. Every
company-internal channel (a node-exec command, a runtime's session messaging) is a `command`
route in a private route file.

**Host mode** (runs `agent-teams serve` on the target host):

```json
"node-exec-c": {
  "driver": "command",
  "mode": "host",
  "comment": "The runtime's node-exec command; ssh is refused on host-c.",
  "argv": ["{vars.tool}", "node", "exec", "--node", "{host.address}", "--", "{remote...}"],
  "vars": {"tool": "/usr/local/bin/example-runtimectl"},
  "host_address": {"host-c": "host-c.example.net"},
  "remote": "argv",
  "envelope": "probe",
  "interactive": false,
  "timeout_seconds": 60,
  "environment": {"pass": ["EXAMPLE_TLS_CERT", "EXAMPLE_TLS_KEY"], "set": {}},
  "outcome": {
    "pre_exec_failure": [
      {"exit": 2, "stderr_regex": "^error: node .* not found", "comment": "documented: checked before dispatch"},
      {"exit": 3, "stderr_regex": "^error: not authorized", "comment": "documented: checked before dispatch"}
    ]
  },
  "capabilities": {"max_argument_bytes": 131072}
}
```

**Templates.** Placeholders are drawn from a fixed set: `host.name`, `host.address`, `vars.*`,
`contact.*`, `message.id`, `message.key`, `message.bytes`, `sender.team`, `sender.session`,
`idempotency_key`, `envelope_base64`, `body_file`, and the splice `{remote...}`. An unknown
placeholder is a load error. A placeholder fills part or all of one argv element and never
causes word splitting; there are no conditionals and no expressions. `remote` says how the
remote command reaches the transport: `argv` (spliced as separate elements) or `shell` (the
transport takes one shell string, such as `sh -c "{remote}"`; agent-teams quotes it itself).
`environment` passes only the named variables (paths, never secrets), the way the chat bridge
scrubs its helpers' environment.

**Outcome.** Exit status 0 with a protocol answer on stdout is `answered`. Only the declared
`pre_exec_failure` entries are `not-delivered`; anything else is `ambiguous`. A route with no
such entries can never prove non-delivery, which is safe: it is pinned on ambiguity and resolved
by a receipt over any route.

**Envelope.** `stdin`, `argument` (`envelope_base64`), or `probe`: `teams discover` sends a
known string and records whether stdin arrived (2.4 left this unverified), and the route uses
stdin only when it did.

**Session mode** (an agent-runtime session as the destination, 10.5) and **relay mode** (a chat
post, 10.6) use the same machinery with named operations instead of one argv.

### 10.5 Session routes: agent-runtime sessions such as agentcloud

A session contact is used when the coordinator is a runtime session that no host-side
agent-teams serves (I1's coordinator, before anyone registered it). When the session is
registered (`agentctl attach-self`, or `agent-teams init --coordinator-self` inside it), the
host contact is preferred: that host's agent-teams verifies it and its deliverer sends through
the runtime locally.

```json
"cloud-session": {
  "driver": "command",
  "mode": "session",
  "comment": "Session-to-session messaging of the company's agent runtime.",
  "operations": {
    "deliver": {
      "argv": ["{vars.tool}", "send-message", "--session", "{sender.session}",
               "--to", "{contact.session}", "--idempotency-key", "{idempotency_key}", "--body-file", "{body_file}"],
      "outcome": {"pre_exec_failure": [{"exit": 2, "stderr_regex": "^usage:"}]}
    },
    "inspect": {
      "argv": ["{vars.tool}", "inspect", "--session", "{contact.session}", "--json"],
      "fields": {"exists": "/exists", "owned": "/owned_by_caller", "halted": "/halted",
                 "archived": "/archived", "title": "/title"}
    },
    "status": {
      "argv": ["{vars.tool}", "message-status", "--session", "{contact.session}", "--idempotency-key", "{idempotency_key}", "--json"],
      "fields": {"known": "/found", "state": "/state"},
      "comment": "Omit if the runtime has no such query; the contact is then pinned on ambiguity."
    }
  },
  "vars": {"tool": "/usr/local/bin/example-runtimectl"},
  "identity": {
    "sender": {"requires": "session", "session_from_env": "EXAMPLE_SESSION_ID"},
    "target_check": {"field": "title", "equals": "{contact.expect_title}"}
  },
  "attribution": "transport",
  "authority": "peer",
  "idempotency": {"native": true, "scope": "target-session", "same_key_other_body": "first-wins",
                  "window_seconds": 86400, "settle_seconds": 120, "keyless_retry_may_duplicate": true},
  "capabilities": {"stdin": false, "max_body_bytes": 65536, "arrives_mid_turn": true,
                   "expiry": false, "cancel": false, "halted_detection": true}
}
```

What the code does with each field, without knowing what runtime it is:

- **`identity` and `attribution`.** `transport` means the runtime itself names the sending session
  to the receiver, so the sender is proved by the runtime (8.2); `header` means the runtime
  attributes the message to the owner or a generic tool, so the only attribution is the rendered
  header, labelled "asserted". `target_check` is the identity check of the destination, from the
  `inspect` fields (question 7).
- **`authority`.** `peer` (an ordinary message) or `operator` (a directive, such as a runtime's
  system-notification mode). The code refuses an `operator` route unless the sender is a
  registered service *and* the receiving team's declaration grants that service `authority:
  operator` (question 6). Coordinators never send with operator authority.
- **`idempotency`.** Whether the runtime takes a key, its scope and window, and what it does with
  a reused key and a different body. With `same_key_other_body: first-wins` the code refuses to
  reuse a key with a new body (8.1 already does). `keyless_retry_may_duplicate` forbids any retry
  without the key.
- **`capabilities`.** `expiry: false` and `cancel: false` mean no tombstone can stop a message
  once delivered to the runtime, so the sender checks `not_after` and the topic rule from its own
  outbox immediately before `deliver`, and refuses `--kind hold|resume` unless
  `--allow-unexpiring` is given. `halted_detection` makes the sender run `inspect` after
  `deliver` and report `journaled-halted`. `max_body_bytes` bounds the body; `stdin: false` means
  the body goes by `body_file`.
- **Verification at delivery time.** `inspect` before `deliver` (exists, owned, not halted or
  archived, title matches) and after it. This is the runtime's own record of its session, the
  analogue of the receiving host's agentctl (principle 1); it is not a remote reading of panes.

### 10.6 The chat relay: an opportunistic route between coordinators

The owner: "explore whether any chat bridge could be used as another route between coordinators,
but it would have to be opportunistic, because we need to make sure our components can be used
individually."

**Shape.** Both teams' coordinators already have chat bridges. The owner creates a shared
channel (say `#coord-relay`) and marks it as a **relay channel** in each bridge's configuration.
A message travels:

1. On host-b, `agent-teams send tools` (or `pump`) picks route `relay-coord`. Its `post`
   operation runs host-b's chat outbound helper (the same helper the bridge uses, named in the
   route's data) to post into the channel, as host-b's bridge app, one post containing a short
   human-readable line and the envelope as compact JSON in a code block. The helper's operation
   UUID is derived from the message ID, so the helper's own idempotency applies.
2. On host-a, the bridge sees the post in a relay channel from an admitted **peer sender**
   (below). It does not type it. It pipes the post's text and the poster's user ID to the relay
   channel's one configured command, `agent-teams relay-in --route relay-coord --peer-sender
   <user id>`. `relay-in` dispatches on the post's type: an envelope goes to `accept --via
   relay:relay-coord`, which does everything a host route would (verify, refuse a wrong
   `to.host`, claim in the ledger, hand to the deliverer); a receipt updates the local outbox.
3. host-a's agent-teams posts the receipt JSON in the post's thread through its own relay
   route, as a post of type `receipt`; host-b's bridge, which admits host-a's bridge the same
   way, pipes it to `relay-in`. **A receipt never draws a receipt**: `relay-in` answers only
   envelopes, so two bridges cannot ping-pong.

**What has to change in the bridge, and why.** Today the relay cannot work: the Slack provider
drops `bot_message` subtypes and every sender outside `allowed_senders`, and a bridge's posts
come from its app's bot user (2.5). Adding the peer bot to `allowed_senders` is not an option:
that list means "the owner", and the peer's posts would become owner-authority prompts typed into
the coordinator. So a relay channel needs a separate, narrower admission class, generic enough to
belong to the bridge rather than to teams:

- a `relay_channels` entry in the bridge configuration: `{channel, peer_senders: {<user id>:
  <label>}, command: [argv]}`;
- in a relay channel, only posts from listed peer sender IDs are admitted (and, if the
  provider delivers the peer's posts with the `bot_message` subtype, that subtype is admitted
  there only; which form arrives is unverified and the enrollment probe records it);
- admitted relay posts are never typed and never prompts; they go only to the one configured
  command, on stdin, with the poster's ID as an argument;
- owner messages in a relay channel are not requests (a relay channel is not also an owner
  channel); the bridge reacts to them as refused.

A provider that cannot deliver posts from other apps cannot carry a relay, and the route is
simply not offered there.

**Attribution.** The receiving declaration's relay entry maps each peer sender ID to the one
team it may speak for (`"peers": {"U0PEER": "sim"}`); `accept --via relay` refuses an envelope
whose `from.team` is not the team mapped to its poster, so one team's bridge cannot post as
another team. The receiver labels the sender "asserted by sim's bridge (app user U0PEER) in
#coord-relay". The chat provider vouches that the post came from that bot user, which a person
in the channel cannot forge; the agent-level claim inside the envelope is the sending host's
assertion, as on other remote routes. If two teams' bridges post as the same app, the poster
identifies only "one of the owner's bridges"; one app (or bot user) per team is recommended
(question 15).

**Dedupe.** Relay posts can appear more than once: the sender's helper may repost after an
uncertain outcome (2.5), and a bridge's rescan or a replayed stream can present one post twice.
None of that matters for delivery, because every copy carries the same ID and ends at the same
ledger (9.2); the channel may show a duplicate post, the coordinator never sees one. The
helper's repost cannot be switched off today (`--uncertain-repost-after-seconds` accepts
30-86400; `embedded_chat_userguide.md:2251`), so the relay route sets it to the maximum, and a
`--no-uncertain-repost` mode is proposed for the helper. Because the operation UUID is derived
from the ID, a resend over the relay returns the helper's recorded post and never posts again:
**the relay route cannot retry**. An ambiguous relay post is resolved by the receipt in the
thread or by a receipt over a host route, and a resend after "unknown" goes over another route
to the same contact.

**Limits.** One post per envelope: an envelope larger than the route's `max_envelope_bytes`
(below the helper's 11,000-byte chunk size) is a definite `not-delivered` at the sender, nothing
is posted, and the walk moves on. Multi-part posts are where I5 went wrong, and a relay envelope
is never split. Bodies are visible to every channel member, so `--private` messages never take a
relay route (question 16). Latency is that of two bridges; if host-a's bridge is down, the post
waits in the channel and may be read after `not_after`, in which case `accept` records it
`expired`.

**Why it stays optional.**

- Each bridge works exactly as today without a `relay_channels` entry, and agent-teams works
  without any chat bridge; neither binary links the other (4.3).
- A relay route is offered only when the sender's route data names it, the receiving host's
  declaration lists it in `relay_channels`, and the last probe saw both bridges running with
  that channel. Otherwise it is skipped, which is definite.
- It ends at the same `accept` and ledger as every other route, so it can be used as a fallback,
  or with a host route for receipts and fences: a fence written over ssh also bars a relay post
  the bridge has not read yet (9.4).

## 11. Route selection, fallback and failure reporting

### 11.1 The walk

For a message to `TEAM[@HOST]` (entry chosen as in question 2):

1. **Contacts in declaration order.** For each contact not yet released or pinned elsewhere:
2. **Candidate routes.** For a `host` contact, the entry host's `routes` list in order; for a
   `session` contact, its one route. Drop, without an attempt (each a definite skip, recorded):
   routes whose driver is unavailable here, whose link is known down, whose capabilities do not
   fit (envelope too large, stdin required but absent), which expose the body when the message
   is `--private`, and relay routes the receiver has not advertised.
3. **Attempt** `accept` (or `deliver`) over the first candidate.
   - `answered: accepted` or a known state: pinned to this contact; done for now.
   - `answered: refused`: definite for this attempt, and durable at the receiver (9.4). A
     policy or conflict refusal ends the message as `refused`, since another contact of the same
     team would apply the same policy. A refusal about the contact (verification,
     `remote-too-old`) releases it only under 9.4's rule: if any earlier attempt to it was
     ambiguous, a ledger-backed refusal already acts as the fence, and `remote-too-old` (no
     ledger) leaves the contact pinned.
   - `not-delivered`: try the next candidate route for the same contact.
   - `ambiguous`: stop the walk. Resolve by `receipt` over the candidates in order (9.3); unknown
     leads to a resend to the same contact; no answer leaves the message `ambiguous` for `pump`.
4. **Next contact** only when the current one is released: every attempt definite and
   undelivered, or fenced (9.4).
5. With no contact left, the message is `deferred` (some route might come back) or `refused`
   (every contact refused); `pump` retries `deferred` until `not_after`.

Two `pump` processes never work one message at once: each outbox entry has its own lock, and the
attempt record is written before the route runs (the helper's rule in 2.3).

### 11.2 Default order

The owner's file orders routes per host; the recommended default puts the cheapest verified route
first: `local`, then a persistent ssh link, then plain ssh, then a host-mode command route, then
a relay. Session contacts come after host contacts. A relay is never first unless it is the only
route, because its latency is two bridges and its receipts are slower.

### 11.3 Failure reporting

- `send` prints the walk as a table: contact, route, verb, phase reached, classification, reason,
  and age of the route's health state. For example: `host@host-b link-b skipped (link down since
  12:40Z, 3 failures)`, `host@host-b relay-coord posted; accepted 13:00:12Z`.
- Receipts carry the receiver's findings (stale-bridge, shared-pane-bridge, `queued-no-drainer`,
  `hook-only`, backlog depth and age of the oldest pending item).
- `teams check` shows per-route health: link state, last success, last definite failure, last
  ambiguous attempt.
- `team-log.jsonl` on each side records every attempt and check (13).
- Exit status follows section 16's table; the JSON `state` is the full answer.

### 11.4 What each route verifies, and by whom

| Route | Coordinator verified by | Sender proof as rendered | Ambiguity resolved by | Cancellation after hand-over |
|---|---|---|---|---|
| `local` | this host's agent-teams and agentctl (8.3) | re-derived by the receiver | `receipt` (a child killed or timed out after the banner is ambiguous too) | tombstone (12.2) |
| `ssh` (with or without link) | the receiving host's agent-teams and agentctl | "proved on host-b, via the host-b key" | `receipt` over any route; `fence` | tombstone |
| `ssh` pull | the puller's own agent-teams and agentctl | "pulled from host-a's outbox over link-a" | re-offer of the same ID; ack | tombstone |
| `command`, host mode | the receiving host's agent-teams and agentctl | "proved on host-c, via node-exec-c" | `receipt` over any route; `fence` | tombstone |
| `command`, relay mode | the receiving host's agent-teams and agentctl, invoked by its bridge | "asserted by sim's bridge in #coord-relay" | receipt in thread, or over a host route; `fence` over a host route | tombstone |
| `command`, session mode | the runtime's own `inspect`, before and after | the runtime's attribution, or "asserted" | the route's `status` operation if declared; else pinned | none (runtime has no expiry or cancel) |

## 12. Agents that only take input when idle

I3 is not a transport failure; the message was delivered exactly as designed, hours after it meant
anything. The fix belongs in the queue, and it helps local sends too. These are generic queue
features of the shared library and agentctl's drain (4.3); they know nothing about teams except
that some items carry the fields.

### 12.1 Expiry, topics, supersede, retract

- **Expiry.** A queue item may carry `not_after` (Unix ms). Every team message has one: `--ttl` or
  `--expires`, else the receiving team's `default_ttl_seconds`, else 2 h, never beyond
  `max_ttl_seconds` (9.2).
- **Topics, by the sender's order.** `--topic SLUG` makes the newest message in a topic from one
  sender team supersede older ones from that team not yet typed. "Newest" is the sender's
  `(created_at, host, seq)`, not arrival order. A message older than the newest message the
  ledger has **taken for delivery** on its topic (`claimed`, `accepted`, `typing`, `presented` or
  `delivered`; never a refused, fenced or expired one, which nobody got) starts `superseded` and
  is never queued, so a `hold` deferred at 13:00 and delivered at 13:10, after a `resume` created at 13:05
  was typed, is dropped. Across hosts, two messages within the skew tolerance (default 2 s) are
  both delivered, with a warning. Local `agentctl send --topic` uses the namespace
  `local:<sender>`.
- **Explicit supersede.** `--supersedes KEY` cancels a specific earlier message if not yet typed;
  too late is reported.
- **Retract.** `agent-teams retract ID` cancels the message if not yet typed.
- **Visible backlog.** `accept` reports queue depth and the oldest pending item's age, and `send`
  prints it ("3 messages already waiting, oldest 2 h; target is idle-only"), the signal I3
  lacked.

### 12.2 Cancellation without the delivery lock

A drain holds the delivery lock while it waits for a busy pane (2.1), exactly the I3 window, so
cancellation is a **tombstone**: `queue/cancel/<id>.json`, written by atomic rename with no queue
lock. The drain re-reads an item's tombstone, `not_after` and (for `tm-*`) ledger state **after**
readiness and immediately before the paste, under the target lock, and moves a cancelled,
expired or duplicate item to `queue/cancelled/` with the reason instead of typing it. A tombstone
that arrives after the paste is too late, and the receipt says so. A new message on a topic
writes the tombstones for older ones before it is itself enqueued.

### 12.3 Who drains, and never `accept`

`accept` claims, hands over and returns; it never types. Typing from `accept` would hold the
delivery and target locks across a readiness wait inside an ssh session that can drop, and would
type any chat prompt ahead of it without the bridge's read-before-typing check. Team items in an
agentctl queue are typed by the drainers that already serve it (a bridge, `watch-agents`, a human
`agentctl send`) once their binaries understand the lane (12.4), or by `agent-teams drain` from a
timer on hosts with no such drainer. `agent-teams drain` calls the shared library's drain with
bounds (`--deadline`, `--lock-timeout`, `--max-items`, the flags the helper in 2.3 asks for, added
to agentctl as a prerequisite), types only `tm-*` items, and only while the head of the queue is
one; with a chat prompt ahead, it leaves the queue to the bridge and reports
`waiting-behind-chat`. Whether this local typing should live in `agent-teams drain` or only in
`agentctl drain --only-prefix tm-` is question 13's second half; either way it is the receiving
host's own process (principle 1).

### 12.4 Older drainers must not see what they cannot honour

Old binaries ignore unknown JSON fields, so a marker inside an item refuses nothing: a pinned
bridge, watcher, helper or rollback binary would type an expired `hold`. Items with team fields
therefore go to a new lane, `queue/inbox.v2/`, which binaries before phase 2 never list. New
drainers merge `inbox/` and `inbox.v2/` in enqueue order. Until a queue's drainers are new, its
`inbox.v2/` items are typed only by the bounded team drain, and `accept` says so in the receipt
when it finds a running drainer whose binary predates the lane (`agentctl capabilities` on that
path). Every consumer that hardcodes the four phases must learn `inbox.v2/`, `cancel/` and
`cancelled/`: agentctl's enqueue duplicate check and `message_state` (`agent.rs:1422`, `1839`) in
both editions; the Python chat runtime's re-enqueue of a `received` item it cannot find
(`py/agentctl/chat_runtime.py:784-797`); and the helper in 2.3. Local `agentctl send
--expires/--topic/--supersedes` is enabled only after that inventory covers every writer.

## 13. Safety

- **Single typist, local only.** Team messaging adds no typing path and no remote one. Its only
  effects are a ledger entry and a hand-over to a local deliverer (typed later by the receiving
  host's drain under the host-wide target lock, or read by a hook), or a runtime journal entry.
  No route reaches a remote Herdr (principle 1).
- **No duplicates by construction** (9): one ID, a durable claim before hand-over, no second
  contact without proof, no resend without asking, no second paste.
- **No impersonation.** Senders are coordinators proved by ancestry or registered services proved
  by cgroup (8.2); remote proofs are labelled with what the route authenticated; the owner is
  never a sender; operator authority needs an explicit grant to a service; body text cannot
  forge a header or a chat reply marker.
- **Least access between hosts.** No ambient ssh: a dedicated key per host pair, restricted by a
  forced command to the protocol's verbs, with its own known-hosts file (10.3). Command routes
  pass only named environment variables.
- **Scoping.** The receiving declaration's `accepts_from` (default nobody), `may_send_to`, size
  and rate limits.
- **Threat model.** Every host runs as the same owner, and anything that can run `accept` on a
  host could write its queue files directly. The checks protect against confused and stale agents
  and scripts, and the restricted keys limit what a leaked key can do; neither protects against
  a hostile process under the owner's account on the receiving host.
- **Logging.** Both sides' `team-log.jsonl` record ID, key, sender and proof, target, contact,
  route, each check and attempt with its classification, outcome, body SHA-256 and byte count.
  The body stays in the outbox, ledger-adjacent queue item or inbox, all private (0700), not in
  the log.

## 14. Chat bridges beside a coordinator

- **Bridges sharing the coordinator's queue.** Most coordinators with a bridge share its queue.
  The receiver checks the running bridges against what it expects (7, 8.3). A team message is not
  a chat request: the bridge has no reply route for it, and the footer tells the coordinator to
  answer with `agent-teams reply`, not a fenced chat reply.
- **Visibility.** A sender may mirror a message to a channel for people (`--mirror-to-chat`,
  through a relay-mode route's `post` operation, marked as a mirror so no bridge relays it). That
  is for people only, never a delivery, and never posted as the owner.
- **Bridges sharing a pane (I2).** Every running bridge whose agent resolves to the same pane must
  have its own `reply_alias_range`, with no two overlapping. `discover`, `check` and receipts
  report violations; separately, `chat run` should refuse to start when another running bridge
  resolves to the same pane and either has no range or the ranges overlap. This belongs to the
  bridge, whichever binary it ends up in.

## 15. Migration

### 15.1 From a per-host coordinators file

The file in 2.3 maps onto `teams.json` as one team with one entry per host:

| coordinators/v1 | teams.json |
|---|---|
| `hosts.<short>` | `hosts.<short>` and an entry `{host: <short>}` in the project's team |
| `fqdn`, `ssh` | an `ssh` route in `routes.d` (with `use_user_ssh_config: true` until `ssh enroll` has run); a `command` route by hand where ssh is refused |
| `herdr` | dropped: agent-teams never calls Herdr; the receiving host's agentctl knows its own |
| `coordinator.name` | the entry's `coordinator` (informational) |
| `coordinator.harness` | dropped; the declaration designates the coordinator |
| contact `agentcloud` `session`, `url` | a `session` contact with a session-mode route; URL dropped |
| contact `agentctl` `agent`, `registry` | a `host` contact; `registry` goes into that host's declaration (`deliverer.registry`) |
| `bin`, `herdr_bin` | `hosts.<short>.agent_teams` |
| `bridge_unit` | dropped from the sender's data; the receiving host's probe finds bridges |
| `workspace`, `tab` | dropped; the receiving host reports them |
| contact `herdr` | not mapped; reported and dropped |

`agent-teams import --coordinators-file FILE --team NAME` prints the converted files for
review; it does not write.

The helper becomes a thin wrapper: it keeps its command line, dedupe and `--repeat-after` memory,
digests and `resolve`, and replaces its transport with

```
agent-teams send <team>@<host> --from-service tick-hub --key <key> --ttl <t> --kind info \
    --allow-undeclared --file - --json
```

mapping states onto its outcomes (`delivered`, `accepted` → queued, `deferred` and `ambiguous` →
deferred, `possibly_submitted`, `lost`, `refused` → not-enqueued, `expired`/`superseded` →
expired). `--allow-undeclared` lets it work on hosts with no declaration yet: an explicit
registry from the import is used, the receipt and both logs carry `unverified-team`, and the
option is removed one release after phase 5. Its alerts to agentcloud sessions arrive today as
`[operator]` directives and would become peer-level unless the receiving team grants it operator
authority (question 6).

### 15.2 From revision 1

Nothing was implemented, so only the documents change: `teams.yaml` becomes `teams.json` plus
`routes.d`; `REGISTRY/team.json` becomes `teams.d/<team>/declaration.json` plus the registry's
own coordinator designation; `reach: local|ssh|exec` becomes named routes; the `agentcloud`
contact kind becomes a `session` contact on a data-described route; the `chat` contact becomes a
relay route; `agentctl team(s) ...` becomes `agent-teams ...`.

## 16. CLI summary

```
agent-teams show [TEAM] [--json]                        # parsed files + freshness
agent-teams check [TEAM[@HOST]]... [--strict] [--json]  # verify contacts and routes
agent-teams discover [--host H]... [--apply] [--json]   # probe hosts, propose a diff, findings
agent-teams import --coordinators-file FILE --team NAME # convert, print only

agent-teams init TEAM (--registry R | --coordinator-self) [--deliver agentctl-queue|hook-inbox|command]
agent-teams send TEAM[@HOST] (--file F | --text T) [--key K] [--topic T] [--supersedes K]... [--ttl D | --expires TIME]
    [--kind K] [--private] [--mirror-to-chat ROUTE] [--from-service NAME] [--allow-undeclared] [--allow-unexpiring] [--wait S] [--json]
agent-teams reply ID --file F [--key K]
agent-teams retract ID
agent-teams status ID | outbox | pump [--retry-refused] | pull --for TEAM
agent-teams relay-in --route R --peer-sender ID              # (target) the relay channel's one command: envelopes and receipts
agent-teams seen ID                                           # local ledger: has this ID been delivered here?
agent-teams redeliver ID                                      # human only: show a delivered message again, marked as such
agent-teams hook                                              # harness hook for the hook-inbox deliverer
agent-teams drain [--deadline S] [--lock-timeout S] [--max-items N]   # (target) types tm-* only

agent-teams ssh enroll --peer HOST                            # make a key; print the authorized_keys line
agent-teams link run ROUTE | up ROUTE | down ROUTE | status [ROUTE]

agent-teams serve --stdio --peer HOST                         # (target) forced command: accept, receipt, fence, probe, ping, pull, ack
agent-teams accept --protocol 1 [--team T] [--via relay:ROUTE --peer-sender ID] (--envelope - | --envelope-base64 B)
agent-teams receipt ID | fence ID | probe --json              # (target) receipt and probe are pure reads; fence takes the ledger lock

agentctl send NAME --file F [--expires TIME] [--topic T] [--supersedes ID]   # generic queue features, after 12.4
```

Global: `--teams FILE` (default `$AGENT_TEAMS_FILE`, then
`$XDG_CONFIG_HOME/agent-teams/teams.json`) and `--routes DIR`.

Exit status extends `agentctl send`'s 0, 75 and 76 with 65 for terminal non-delivery, which
`agentctl send` does not need:

| Exit | Meaning | Safe to re-run the same command? |
|---|---|---|
| 0 | `delivered`, or `journaled` for a session contact | yes; it reports the same state |
| 75 | not settled: `accepted`, `deferred`, `awaiting-pull`, or `--wait` ended first | yes; the ID makes it a retry |
| 76 | uncertain: `ambiguous`, `ambiguous-expired`, `possibly_submitted`, `lost` | yes; it asks, it never resends blind |
| 65 | terminal and undelivered: `expired`, `superseded`, `retracted`, `refused` (including every contact refused or fenced), `journaled-halted`, retract too late | yes; nothing new is sent |
| 2 | usage or configuration | n/a |

## 17. Phased plan

1. **P0: extract the shared library.** Queue format, enqueue duplicate check, harness identity
   and ancestry into `agent-queue`, with agentctl switched to it and no behaviour change; the
   Python edition documented against the same format. (Coordinated with the suite-factoring
   lane's audit.)
2. **P1: read-only model.** `teams.json` and `routes.d` parsers with schema and comment handling,
   `teams show`, `probe`, `check`, `discover` (proposal only), `import`. Surfaces I1, I2 and I4
   with no change to delivery.
3. **P2: queue lanes and cancellation** in the library and both agentctl editions: `inbox.v2/`,
   `cancel/`, `cancelled/`, `not_after`, the pre-paste re-check, bounded drain flags, the 12.4
   inventory, with cross-edition differential cases.
4. **P3: exactly-once core and local delivery.** Declarations, ledger with claim, fence and
   recovery; `accept`, `receipt`, `fence`, `serve`; deliverers `agentctl-queue` and
   `hook-inbox`; `send`, outbox, the walk, `status`, `pump`, `retract`; sender proof
   (coordinator, service) with a test per mode; the `local` route. **The model-based harness of
   9.7 lands in this phase, before any network route.**
5. **P4: network routes.** The `ssh` driver with `enroll`, the restricted forced command and its
   own known-hosts; links (`on-demand`, `persistent`) and pull; the `command` driver in host and
   session modes; route health; the harness extended with each driver's fault model.
6. **P5: replies, policy, migration, relay.** `reply`, `pull`, `accepts_from` with kinds,
   operator grants, limits, retention; the helper in 2.3 as a wrapper; the bridge's generic
   relay-channel admission and the relay route; the shared-pane range check in `chat run`.

P1 and P2 are independent of each other once P0 is done. Fixing I3 for ordinary local sends waits
for P2 and its inventory.

## 18. Questions for the owner

Numbers 1-11 keep revision 1's numbering; 12 onward are new.

**Resolved**

- **Format** (new): JSON, with explicit `comment` and `notes` fields and a versioned `schema`
  (6.1). Owner, 2026-10-24.
- **8. Workers as senders:** only coordinators, plus registered services, which this design keeps
  explicitly for operational alerts (8.2). Workers neither send nor receive. Owner, 2026-10-24.

**Open**

1. **Where do `teams.json` and `routes.d` live across hosts?** Recommendation: a copy of
   `teams.json` and the public routes on every host from a dotfiles repository; private route
   files (question 17) from a private one. Senders work from their local copy; staleness is handled by
   the checks.
2. **Default entry** for `send proj` when a team has entries on several hosts. Recommendation:
   the sender's own host, then `primary`, then refuse.
3. **May `accept` ever type?** Principle 1 would allow it (it runs on the receiving host), but
   the design says never (12.3), at the cost of a drainer on each receiving host.
4. **Expiry.** 2 h default without `--ttl`? Shorter for idle-only targets (Muse)? A maximum TTL of
   7 days, with no `--ttl none` (9.2 needs a bound)? Should a `hold` to Claude Code use
   `--deliver-while-working`?
5. **Default scoping.** `accepts_from` defaults to nobody, with `service:` senders listed
   explicitly. Recommendation: yes.
6. **Operator or peer for services.** Recommendation: peer, with an explicit per-service
   `authority: operator` grant in the receiving declaration for alerts that must stay directives.
7. **Session identity.** Is a session title match (`target_check`) acceptable, or must a session
   coordinator register (`attach-self` or `init --coordinator-self`) before it can be a contact?
9. **Retention.** Terminal ledger entries kept `max_ttl + 30 days` past creation (so pruning can
   never reopen an ID)? And should this work also prune agentctl's `processed/`?
10. **Clock skew.** A 2 s cross-host tolerance for topic order, or require one sending host per
    topic?
11. **Editions.** `agent-teams` in Rust only; the queue changes of P2 in both agentctl editions.
12. **ssh access.** A dedicated key per host pair, restricted by a forced command to the
    protocol, installed by you by hand (recommended)? May routes opt into your own ssh
    configuration and agent during migration (`use_user_ssh_config`), and should that opt-in
    expire?
13. **Where it lives.** A separate `agent-teams` binary sharing an extracted `agent-queue` crate
    (recommended, section 4), or subcommands of agentctl? Which name? And should the bounded
    local typing of team items be `agent-teams drain` or `agentctl drain --only-prefix tm-`?
14. **Links.** Which host pairs get a persistent link (a user unit you install) versus on-demand
    connections? For pairs reachable in one direction only, is the pull link (10.3) acceptable?
15. **Chat relay.** Do you want a shared coordinator channel at all? If so: one bridge app (or
    bot user) per team, so the poster identifies the team; and the bridge change of 10.6 (a
    relay-channel admission class, separate from `allowed_senders`, whose posts are never typed).
16. **Bodies in a channel.** May non-private message bodies appear in a relay channel, or should
    the relay carry only the ID, with the body fetched over a host route?
17. **Private route files.** Where should company-internal route definitions live, and should the
    public repository ship only placeholder examples (recommended)?
18. **Hook-only teams.** Is the `hook-inbox` deliverer acceptable for teams without agentctl,
    given that it never wakes an idle agent (4.4)?
19. **Unresolvable ambiguity.** A message pinned to a contact that never answers ends
    `ambiguous-expired`. Should a human-only `agent-teams resolve ID (--fence | --mark-delivered)`
    exist, run on the receiving host when it is reachable again?
20. **Test budget.** How many seeds of the model-based harness per gate run (a bounded run,
    under a minute), with a longer soak where?

## 19. Related designs

Store-and-forward mail (an outbox, per-hop receipts, delivery status notifications) is the closest
analogy for the outbox and receipt states; message expiry plays the role of a delivery deadline.
Matrix and XMPP federation route on a sparse address (`user@server`) and leave authoritative state
with the server that owns it, as `teams.json` leaves it with the host's declaration. The
exactly-once design is the familiar idempotent-receiver pattern (a durable deduplication table
keyed by message ID, as transactional outboxes and inboxes use, and as message brokers do with
per-producer sequence numbers); the fence is a write-once tombstone in the same table. Brokers
with per-message TTL and "last value" topics (NATS JetStream per-subject limits, Kafka log
compaction) are the precedent for topic superseding. autossh and OpenSSH's ControlMaster are the
precedent for the managed link, and forced commands in `authorized_keys` (as Git hosting and
rsync-only backups use) for the restricted key. Gas Town's `gt nudge` falls back to a file queue a
hook drains at the next turn boundary, the model for the hook deliverer; CLI Agent Orchestrator
marks a message delivered before the paste so a crash cannot deliver twice, the same choice as
agentctl's `inflight/` rename (`common/docs/agentctl/RELATED_WORK.md:123-140`). None of them has
the constraint that shapes this design: the final hop is a terminal composer that only one local
program may type into, and only when the agent behind it is idle. A `RELATED_WORK.md` for
`agent-teams` is required when the tool is created.

## 20. Review record

**Revision 1** (2026-10-24, after one adversarial review of the first draft): `accept` stopped
draining (it held locks across a readiness wait inside ssh and would type chat prompts without the
bridge's check); cancellation became a lock-free tombstone re-checked before the paste; team items
moved to a lane old drainers never list, with the phase-hardcoding consumers inventoried; topic
order follows the sender's clock; a per-team receiver ledger replaced the queue as the dedupe
record, adding `lost`; `--from-service` requires a registered unit and cgroup membership and proof
is taken at outbox write; the contact walk advances only on a definite answer and pins once handed
over; keyless keys derive from content and replies default to `reply:<ID>`; agentcloud contacts got
their own states; the bridge check reads the main process and bridge state; "anchor" was renamed
"declaration"; remote binaries must be absolute paths, the envelope falls back to an argument, and
exit codes were aligned with `agentctl send`.

**Revision 2** (2026-10-24, owner feedback, then one adversarial review):

- `teams.yaml` became `teams.json` plus `routes.d/*.json`, with explicit `comment`/`notes` fields
  preserved by machine rewrites and versioned schemas; the declaration moved to
  `teams.d/<team>/declaration.json` (6).
- Only coordinators and registered services send; workers neither send nor receive; `accepts_from`
  lost the `team/agent` form; replies go to the coordinator; old question 8 resolved (8.2).
- Exactly-once became its own section: one ID end to end, a durable claim before hand-over with
  retention tied to the maximum TTL, ask before resend, a fence before any second contact, no
  second paste, what a coordinator sees on a second sighting, and a required model-based fault
  harness; incidents I2 and I5 named (9).
- The principle that only the receiving host's agentctl does TUI work and verification is stated,
  and the probe no longer reads remote panes (3, 7).
- Routes became a pluggable layer: drivers `local`, `ssh`, `command`; ssh without ambient keys
  (dedicated restricted keys, its own known-hosts, an optional managed link, pull for one-way
  pairs); the `command` transport in host, session and relay modes, with agentcloud-like channels
  described entirely in data; an opportunistic chat relay, including why the Slack provider's
  admission rules block it today and the generic bridge change it needs (10); route selection,
  fallback, failure reporting and per-route verification (11).
- Team messaging moved out of agentctl into a separate tool sharing an extracted library, with
  deliverers for users without agentctl (4); revision 1's `agentcloud` and `chat` contact kinds
  became a `session` contact and a relay route; owner questions 12-20 were added (18).
- The review of revision 2 (15 findings, all code citations confirmed) changed: the ID now
  hashes sender kind, sending host, target as addressed and key, in agentctl's ID alphabet (8.1);
  claims name their hand-over target, the drain updates the ledger before and after the paste,
  and recovery never re-hands over without proof of absence, including after a queue is archived
  (9.2, 9.5); non-idempotent deliverers recover to `possibly_submitted`; every refusal is a
  durable fence, and successor IDs replace reopened ones (9.4); `accept` refuses a wrong
  `to.host`, and pull is a step of the walk filtered by host (9.2, 10.3); the session `status`
  rule counts "unknown" only within the window after a settle time (9.4); the `inbox.v2/` enqueue
  is lock-free and `receipt` never recovers (9.2); the relay has one dispatching command, no
  receipt loops, peer-to-team binding, and cannot retry (10.6); the hook deliverer has an
  uncertain `presented` state (9.5); the worker restriction's limit under cgroup-only isolation is
  stated (8.2); topic supersede counts only messages taken for delivery (12.1); the state table,
  exit table and CLI were completed (8.4, 16).

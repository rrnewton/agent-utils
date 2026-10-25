# agentctl-team-messaging: messaging another team's coordinator, across hosts, exactly once

Working design, 2026-10-24. Not user documentation, and nothing here is implemented.
`#496 agentctl-team-messaging`.

**Revision 2** (same day) follows the owner's feedback on revision 1: the data files are JSON;
only coordinators (and registered services) send to other teams; routes are a pluggable layer
described in data (local, ssh with an optional managed link, a generic command transport for
company-internal channels, and an opportunistic chat relay); exactly-once delivery is a
first-class section; only the receiving host's own agentctl touches its panes; and team
messaging is factored as its own tool. Section 20 records what each revision changed.

**Revision 3** (2026-10-25) folds in the owner's answers to questions 12 (second half), 13, 14, 15
and 17-20, and the owner's approval of Rust only for question 11. It adds a topology: a star of
persistent links that spokes dial out to, instead of a link per host pair; a store-and-forward relay
at the hub that keeps section 9's guarantees, limited to one relay; and the `jump` option for ssh
routes (11.5-11.7). It adds a related-work comparison with Gas Town and other public systems (19,
and a draft [`RELATED_WORK.md`](2026-10-24-agentctl-team-messaging/RELATED_WORK.md)). It defines one
machine-parsable team tag used on every route (8.1), and makes a direct message to the other
coordinator's chat conversation the first chat route, with the shared coordinator channel deferred
(10.6, 10.7).

Code references are to `main` at `8efb7191` unless marked with `2a9d8012`.

## 0. Summary

A **team** is a coordinator plus the workers it manages: one harness directory, one agentctl
registry (if it uses agentctl), one Herdr workspace, on one host. The owner runs several teams
on several hosts. Today a team's coordinator can be reached from outside its harness only by
someone who knows, by hand, which host it is on, which registry names it, which binary drives
that registry, and whether the chat bridge that shares its queue is still the one running. That
knowledge goes stale, and every incident in section 1 is a delivery made on stale knowledge or a
retry that delivered twice.

The proposal, in eight parts:

1. **A separate, à la carte tool**, `agent-teams` (the owner's name for now, 2026-10-25), not more
   subcommands of agentctl. It shares one small library with agentctl (queue format, ledger
   primitives, harness identity) and integrates with agentctl, chat bridges and transports through
   data and commands, so each piece works alone (section 4).
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
   flags, idempotency semantics, attribution. A chat route (first, a direct message to the other
   coordinator's conversation) is opportunistic, never required, and used sparingly, because the
   owner reads it (sections 10, 11). Every message carries one fixed team tag,
   `[sim host-b -> tools host-a tm-…]`, on every route (8.1).
8. **A star, not a mesh.** Persistent links run from each host to a hub named in data, dialled
   by the spoke, so one-way-reachable hosts work and n hosts need n-1 links. Spoke-to-spoke
   messages go direct when an on-demand route exists, through the hub as an ssh jump, or through
   a store-and-forward relay at the hub that holds custody without deciding anything. One relay
   at most; a general multi-hop mesh is not worth building (11.5-11.7).

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
- **No coordinator designation of its own, and no parent edges** (revision 3, checked at
  `e707ba70`). An agent record carries no creator, parent or controller field: nothing records
  which agent started which. "The coordinator" exists only in scattered, per-consumer places:
  the optional `coordinator` key of `REGISTRY/restore.json`, read by `restore` alone
  (`embedded_userguide.md:578, 611`); the `--coordinator NAME` argument each `watch-agents`
  invocation is given (`cli.rs:278`); the one agent a chat bridge's `chat.json` routes to; and
  wrkslots' per-slot coordinator process ID (2.2), which the caller supplies. They can disagree,
  and none is kept current by `rename` or `adopt`. A user of agentctl alone, without team
  messaging, has no one place that says which agent is the coordinator.
- **No ssh.** agentctl itself never runs ssh today.

### 2.2 wrkslots

`.wrkslots.yml` per harness describes the slot pool; slot ownership is per coordinator process.
Slot boxes (cgroup, user-namespace or root isolation) limit what a worker can write, and its
process ancestry may stop at the box's relay.

### 2.3 A consuming project's per-host coordinators file

One consuming project keeps a file (schema `<project>/coordinators/v1`; it belongs to that
project, and neither agentctl nor this design owns it) that answers "where do operational alerts
go?" per host: a name and optional ssh target, an optional Herdr binary, and
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
| "Who is this registry's coordinator?" | yes, a fact agentctl owns | agentctl's registry-level designation (P0b, 17); teams read it |
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
another controller of agents. **Decided** (question 13, 2026-10-25): option B, named
`agent-teams` for now.

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
  item; 9.2, 9.5), useful to local sends too, and the registry-level coordinator designation of
  P0b (17), which agentctl needs for itself (2.1) and team messaging only reads.
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
integration is by data and commands. The owner accepted this on 2026-10-25 ("Yes and let's just
name it agent-teams for now"). The extraction is the first implementation step (section 17); it
also documents the queue format, which the Python edition of agentctl only has to recognise well
enough to refuse the new lanes (question 11).

### 4.7 Relation to the suite factoring audit

`#498 suite-a-la-carte` recommends (its section 2.4) keeping one `agentctl` command built from
libraries: `agentctl-core` (registry, queue, drainer, target lock, Herdr client, harness TUIs,
subagents, inbox), `agentctl-chat`, and later `agentctl-teams`, with chat and teams as default-on
Cargo features, a CI build of core alone, a check that teams never depend on chat, and an
explicitly versioned registry and queue format.

**Agreed:** team messaging is its own library; it never depends on chat ("no bridge configured"
is normal); the on-disk queue format, versioned, is the contract between binaries; a build
without the feature refuses team commands with exit 2.

**Where this section goes further** (the owner chose the separate binary, question 13):

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
  designation of P0b, 17, which agentctl's `rename`, `restore` and `adopt` keep current once it
  exists; 2.1 says why nothing does today), or by a pinned
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
a route name defined twice is an error. Splitting routes out of `teams.json` lets private
definitions (for a company-internal channel) stay out of anything published while `teams.json`
and the public code stay generic. The formats are in section 10.

**Private route files** (question 17, decided 2026-10-25). Company-internal routes go in files
named `*.private.json`, in the same `routes.d/` or in extra directories, and are never committed
where they could be published:

- **Ignored by name.** Any repository that holds a `routes.d/` (a dotfiles repository, a harness)
  ignores `*.private.json`; agent-utils' own `.gitignore` gets the same line when the tool lands,
  so an example directory in the tree cannot pick one up. `teams check` warns when a
  `*.private.json` file it loaded sits inside a Git working tree and `git check-ignore` does not
  report it ignored.
- **Found by search path.** agent-teams reads `routes.d/` first, then each directory in
  `AGENT_TEAMS_ROUTES_PATH` (colon-separated) and each `--routes DIR` (repeatable), all `*.json`
  files in name order, with one namespace: a route defined twice is still an error. A durable
  copy of the private files may live in a private repository the owner keeps; pointing
  `AGENT_TEAMS_ROUTES_PATH` at that checkout uses it in place.
- **Placeholders only in public.** The public repository ships `*.example.json` route files with
  invented commands (`example-runtimectl`) and `example.net` addresses, which the loader refuses
  to use as live routes.

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
    [--key KEY | --again] [--topic SLUG] [--supersedes KEY]... [--ttl DURATION | --expires TIME]
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
- **Same key, same body** returns the existing message's state from the receiver's ledger,
  but only as a retry: when the key was derived (no `--key`) and the earlier message is already
  terminal and older than the retry window (default 10 minutes), `send` refuses with `refused:
  repeat` ("already delivered at 13:02Z; pass --key or --again to send it again"), exit 65, and
  sends nothing. Otherwise an identical `hold` sent at 14:00, after a `resume`, would return
  `delivered` from 13:00 and never be typed. `--again` derives a fresh key from the derived key
  and the current time.
  **Same key, different body** is refused as a conflict; to change a message, send a new key
  that supersedes the old one.
- **`--private`** keeps the body off routes that declare `"exposes_body": true`: every chat
  route and every hub relay route (11.6).
- **What is typed.** The receiver renders the envelope on its own host:

  ```
  [sim host-b -> tools host-a tm-sim-6f1d0c2a9b7e4d31]
  [team message: kind hold; topic ci-hold; expires 15:00Z; sender proved on host-b; via link-b]
  Please hold landing to main until the mirror is repaired.
  [reply: agent-teams reply tm-sim-6f1d0c2a9b7e4d31 --file FILE]
  ```

  The first line is the team tag (below). The second is generated by the receiver from the verified
  envelope, including how the sender was proved and which route carried it (for a relayed message,
  also the relay: `via link-b, relayed by hub-a`). Every body line whose first non-blank character
  is `[` (so near-misses of the tag such as `→`, capitals or extra spaces too), and lines that look
  like a chat bridge's reply markers, are indented by two spaces, so a body can neither forge a
  header nor be posted by a bridge reading a long idle pane (2.5). The typed text never starts with
  `/`.

- **The team tag: one grammar, every route.** The owner (2026-10-25): "for starters I want just
  dms to other coordinators with clear tagging ... The agents seem to usually want to label their
  inter agent messages but are inconsistent in how they do it." So the label is not left to the
  agent. agent-teams writes exactly one tag, from the envelope, as the first line of everything a
  person or an agent sees: the rendered paste, the hook's print, a session-route body, a chat
  post (10.6, 10.7), and the subject of `team-log.jsonl` records. Agents never write it; a body
  whose lines start with `[` has them indented as above.

  ```
  ; RFC 5234 with RFC 7405 %s: every literal is case-sensitive
  tag      = "[" sender SP host %s" -> " team SP host SP id "]"
  sender   = team / %s"service:" team    ; a team name stands for that team's coordinator (8.2)
  team     = 1*32( %x61-7A / DIGIT / "-" )                     ; ^[a-z0-9-]{1,32}$, as in 5
  host     = ( %x61-7A / DIGIT ) 0*62( %x61-7A / DIGIT / "-" / "." )  ; a teams.json host key
  id       = %s"tm-" ( team / %s"svc." team ) "-" 16( DIGIT / %x61-66 )  ; the ID rule above
  ```

  One line, single spaces, ASCII `->`, no other text inside the brackets. The left endpoint is the
  sender as proved and the sending host; the right is the addressed team and its entry host
  (`to.team`, `to.host`); services send but never receive, so only the left endpoint may be a
  service. Only coordinators and registered services take part (8.2), so no agent name appears; the
  owner's example names the two coordinators by team and host the same way (`[<team-1 coordinator>
  <host-1> -> <team-2 coordinator> <host-2>]`, host names redacted). The receiver re-derives the tag
  from the envelope; a chat post whose tag does not match its envelope is answered `not-recorded`,
  never claimed (10.7). Host keys in `teams.json` must fit `host`; `teams check` reports any that do
  not.

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
| `in-custody` | sender-side only: a hub relay has taken custody (11.6); the final host has not answered yet. Not delivered | the hub forwards it; the sender asks; pinned to the contact |
| `journaled` / `journaled-halted` | session contact: in the session's journal; `-halted` when the runtime shows it will not run | no |

Receipts read the ledger and list queue directories without taking the delivery lock, so
`agent-teams receipt ID` answers at once while a drainer waits on a busy pane. Receipts are
**pulled** by the sender through a route; nothing on the target has to reach back, except where
the only route is the receiver pulling (10.3), in which case the receiver pushes receipts with
its acknowledgement.

**Retention.** Terminal ledger, fence, outbox and hub custody entries are kept until 30 days past
their own `not_after` (9.2); `possibly_submitted` and `lost` until resolved. The queue's
`processed/` is outside this design (question 9).

### 8.5 Replies

`agent-teams reply ID --file FILE`, run by the receiving team's coordinator (proved as in 8.2),
builds an envelope to `reply_to` with `in_reply_to: ID`. Its key defaults to `reply:<ID>`, so
re-running the same reply is a retry and a reply to a duplicate sighting of the same message is
the same reply (9.6). A second, different reply needs `--key`. It travels over the replying
host's routes to the sender's team; where the replying host has none, it waits in the outbox as
`awaiting-pull` and the original sender's host collects it (`agent-teams pull --for host-b`, by
host as in 10.3, never by team alone). A
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
  before claiming and without a ledger entry (a route refusal, 9.4), so a relay post or a pull
  meant for `sim@host-b` cannot be claimed by `sim@host-c`, and a later, legitimate move of the
  message to `host-c` is not barred.
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
- **Every refusal of the message is a fence; refusals of the route are not.** `accept` writes
  its refusal of a message addressed to it (policy, verification such as "the coordinator is not
  live", conflict) as a durable ledger entry for that ID under the ledger lock, so the same ID
  arriving later by any route (a relay post read hours later, a pull) is refused too. A refusal
  of the **route**, decided before any claim, leaves no ledger entry and fences nothing: wrong
  `to.host` (9.2), a relay the final host does not accept (`accepts_relay_from`, 11.6), a hub's
  `hop-limit`, relay-policy or spool-full answer, a chat post answered `not-recorded` (10.7), and
  `remote-too-old` (the host has no agent-teams that understands the protocol). A route refusal
  is definite for that attempt only: the walk tries the next route, it never ends the message,
  and it releases the contact only if no earlier attempt to it was ambiguous. A refused ID
  therefore means a ledger entry at that contact, and nothing else does.
- **Successor IDs, not reopened IDs.** A refused ID is never accepted later on that contact. When
  the cause was transient (the coordinator was restarting) and no other contact delivers,
  `pump --retry-refused` may send a **successor**: the same body under key `<key>~2` (a new ID),
  only when every earlier ID of the message is refused or fenced at every contact it was offered
  to. Each successor is a new message that its predecessors provably did not deliver.
- **A fence** is `agent-teams fence ID --not-after TIME` on the receiving host: under the ledger
  lock, if the ID is unknown it writes a `fenced` entry, so any later arrival of that ID, by any
  route, is refused; if the ID is known it returns its state instead. The sender passes its
  envelope's `not_after`, so the fence is kept as long as any copy could arrive (9.2's
  retention rule); a fence written without one (`resolve --fence` by hand) uses now plus
  `max_ttl_seconds`. A fence is how "the receipt says
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

The hook deliverer claims an item by rename, then re-checks, as the drain does just before the
paste (12.2), its `not_after`, its tombstone and its ledger state, and moves an expired,
cancelled or duplicate item aside instead of printing it; otherwise I3 would recur for hook-only
teams, whose messages wait for the next turn. It records `presented` (an uncertain state) before
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
  running, an old drainer binary beside a new one (12.4), and the Python edition's refusal of a
  queue that uses the new lanes (question 11), checked by the cross-edition differential.
- **Regression cases named for the incidents:** a slow multi-part relay post retried by the
  sender (I5), and two bridges sharing a pane with the coordinator replying (I2).
- **Relay faults (revision 3, for the hub of 11.6).** The model gains one or two hub hosts with
  real spool code, spokes that reach the hub only by pull, and jump routes. Faults: the hub
  crashing before the custody write, after it but before answering, and after a forward attempt
  started; the custody answer lost, so the sender resubmits to the same hub, then to the final
  host directly; the hub's spool lost, or restored from an old copy that still holds entries
  already forwarded; the hub partitioned from the final host for longer than `not_after`; a
  forwarded copy arriving after the final host fenced the ID or after the sender's direct resend
  was accepted; copies of one ID at two hubs; a final host's pull taking an entry and dying
  before its ack; a relayed envelope submitted to a second hub, or back to a hub, or by one hub
  to another; a hub forwarding to the wrong host; a final host that does not accept the hub; a
  trace edited in transit; the hub's clock ahead of or behind the final host's; a jump whose hub
  drops the connection after the envelope was written. Invariants added: no envelope is relayed
  twice (at most one hub in any trace, and never hub to hub); a hub never runs `accept` for a
  team on another host and never writes another host's ledger; a route refusal never leaves a
  ledger entry and never releases a contact after an ambiguous attempt; no contact other than
  the pinned one is tried while any hub holds or may hold a copy, unless the final host fenced
  the ID; custody records outlive `not_after` (9.2's retention rule, applied at the hub); and
  every message that entered custody ends terminal at the final host or `ambiguous-expired`,
  each visible in a receipt.
- **Live trials before reliance.** The owner (2026-10-25), on routes using the owner's own ssh
  configuration: "Sure and no. But good to test any other mechanism we propose." Every mechanism
  other than `use_user_ssh_config` (dedicated forced-command keys, persistent and pull links, jump
  keys, the hub relay, command routes, session routes, the chat route) is relied on only after
  its tests pass **and** a live trial between two of the owner's hosts has exercised it with the
  faults above that can be induced for real (a link killed mid-attempt, the hub stopped holding a
  message, a spoke rebooted), recorded under `ai_docs/transient/` with the commit tested. The
  owner's `teams.json` lists the trials it accepts (`"trials": {"jump": {"record": "<path>",
  "commit": "<sha>"}}`); until a mechanism is listed, `teams check` labels routes using it
  `untrialled` and `send` prints that label. `use_user_ssh_config` needs no trial entry.
- **Budget: the repository's usual split** (question 20, decided 2026-10-25: "repeat the split
  of regression tests plus optional or nightly soak"). The regular gate (`make validate`, when
  agent-teams or the queue library changed) runs a bounded harness pass under one minute: a fixed
  list of seeds that once failed, kept as regression cases forever, plus a few hundred seeds
  derived from the commit being tested, so successive commits explore different schedules and
  every failure is reproducible from the commit and seed printed. The soak is a separate,
  optional target (`make soak-agent-teams SEEDS=… MINUTES=…`) that nobody's push waits for; the
  nightly workflow (`.github/workflows/nightly-all.yml`) runs it for a fixed time after the
  portable contract, uploads each failing seed with its replay command and trace as an artifact
  (90-day retention, as the benchmark series is kept), and reports a red soak through the same
  single open `nightly` issue. A seed that failed in a soak joins the gate's fixed list with the
  fix.

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
  `ConnectTimeout`, `ServerAliveInterval` and `ServerAliveCountMax`.
- **The owner's own ssh configuration: allowed, permanently.** A route may set
  `"use_user_ssh_config": true` to run OpenSSH with the user's configuration and agent (for
  example to inherit a jump host or a host alias). The owner (2026-10-25), asked whether routes
  may do so during migration and whether that should expire: "Sure and no." It is therefore a
  supported mode, not a migration step, and never expires. What changes is attribution: without
  a forced command bound to a key, the remote side runs `agent-teams serve --stdio --peer
  <sender's host>` through an ordinary shell, so `--peer` is the sender's own claim. The receiver
  records it as `peer asserted (owner ssh)`, renders "asserted by host-b over owner ssh", and
  `teams show` labels the route. Dedicated per-pair keys remain the stronger option and stay
  optional (question 12).
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

**Through a hub: the `jump` option.** An ssh route may name another ssh route as its jump:
`"jump": "link-hub"`. The driver then reaches the target with OpenSSH's `ProxyCommand`, running
`agent-teams serve connect <target>` on the hub over the jump route (a new session channel on its
control socket when that route has a link). `connect` is one of `serve`'s fixed verbs: it is
governed by the hub key's forced command like every other verb, allowed only for targets listed
in the hub's `relay.json`, and does nothing but splice its stdin and stdout to the target's ssh
port. So the hub's key line keeps `restrict` and needs **no** forwarding grant: a
`port-forwarding` grant would also open unrestricted `-R` listeners on the hub (`permitopen`
limits only `-L`, `sshd(8)`) and, by recalled behaviour, unix-socket forwarding that could reach
the hub's own Herdr socket, against principle 1. The session to the target is end to end inside
that splice: the target's own key line authenticates the sender's host, the hub sees only
encrypted bytes, and the exactly-once rules are those of a direct ssh route, because it is one.
Under `use_user_ssh_config` the owner's own `ProxyJump` setting may be used instead. A jump works
only when the target accepts ssh from the hub and both are up at once; otherwise the hub relay
(11.6) is the route. The jump's live trial (9.7) must show both forms.

**Hosts where ssh is refused** get no ssh route: a `command` route (10.4), a chat route (10.6,
10.7), pull from the other side if that host can ssh out, or a session contact. With none, `teams
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

**Deferred (revision 3).** Asked whether a shared coordinator channel is wanted, the owner
(2026-10-25): "Not yet. I'll review the design if it's in there. But for starters I want just
dms to other coordinators with clear tagging." The shared-channel design below is kept for that
review and is not in the phased plan. The first chat route is the direct message of 10.7, which
reuses this section's machinery (`relay-in`, peer-to-team binding, receipts in the thread, no
retry over chat) in a conversation with exactly two coordinators in it.

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
   relay:relay-coord --peer-sender <user id>`, which does everything a host route would (verify,
   refuse a wrong `to.host`, check the poster's team, claim in the ledger, hand to the
   deliverer); a receipt updates the local outbox. A post `relay-in` cannot parse is answered
   `not-recorded`, as in 10.7.
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

### 10.7 The chat DM route: a direct message to the other coordinator

The owner (2026-10-25): "for starters I want just dms to other coordinators with clear tagging."
This is the first chat route. It is a `command` route in relay mode whose conversation has
exactly two coordinators in it, and it is as optional as 10.6: no bridge needs it and agent-teams
works without it.

**Where it posts.** Into a conversation the **receiving** coordinator's bridge already watches
and the sending bridge's app can post in. Normally that is the receiving coordinator's own bridge
channel, so the owner sees the message where they already talk to that coordinator. Where that
conversation is a one-to-one direct message between the owner and the receiving app, which a
third app cannot join, the route names a conversation made for the pair (the owner and the two
bridge apps). Never a conversation with a third team's bridge. The route data names the
conversation; whether the provider lets the sending app join or post there, and with what
subtype its posts arrive, is unverified and recorded by the enrollment probe.

**What it posts.** One post per envelope, never split:

````
[sim host-b -> tools host-a tm-sim-6f1d0c2a9b7e4d31]
Please hold landing to main until the mirror is repaired.
```agent-teams-envelope
{"schema":"agent-teams-message/v1","id":"tm-sim-6f1d0c2a9b7e4d31", ...}
```
````

The first line is the team tag (8.1). The body follows for the owner to read; the code block
carries the full envelope, which is authoritative (a provider may rewrite visible text, for
example by escaping `<`, `>` and `&`, so the body hash is checked on the envelope's copy). The
sending agent writes neither the tag nor the block; `agent-teams` renders both.

**What has to change in the bridge.** Two things block this today, and the route is not offered
until both are fixed:

- **The inbound stream never carries the peer's posts.** The provider client's tail emits no
  posts by other bots (an upstream bot-author filter; the plugin notes that "the client
  documents that it emits no posts by other bots", `rs/chat-subscription-slack/src/plugin.rs:961-962`).
  The bridge's rescan lists each channel's history (`plugin.rs:62-77`), and the coordinator
  reported on 2026-10-25 that it does see such posts; this revision did not verify that the
  listing includes bot posts, and the enrollment probe must show it. The rescan's limits bound
  the route: it runs every `rescan_interval_seconds` (90 by default) but only once the stream is
  idle and caught up, or after ten minutes, so a DM can arrive about ten minutes late; it looks
  back only `rescan_window_seconds` (a day by default), so a DM posted while the receiving
  bridge is down for longer is never seen; and it is off when the bridge has no diagnostics log.
  The route therefore declares `"max_ttl_seconds"` no longer than the receiver's rescan window
  (the walk skips it, definitely, for a message whose `not_after` is later), and is offered only
  when the probe saw the receiving bridge with the rescan on. An exception in the upstream filter
  for listed peer IDs would remove these limits (question 26).
- **Admission refuses them.** Stream and rescan apply one admission, which drops `bot_message`
  subtypes and every sender outside `allowed_senders` (2.5). Adding the peer to
  `allowed_senders` would give it the owner's authority. The bridge needs a **coordinator-peer**
  admission class, generic and per conversation: `peer_senders: {<bot user id>: <team>}` and one
  `command`. A post from a listed peer ID, in that conversation only (and with the `bot_message`
  subtype only if that is how the provider delivers it), is admitted to that class and nothing
  else: it is **never typed raw** and never becomes a prompt; its text and poster ID go on stdin
  to `agent-teams relay-in --route R --peer-sender ID`. `relay-in` first undoes the provider's
  text escaping (the bridge's own unescape: `&gt;` back to `>`, and so on) and strips the note
  the rescan appends to a rescued message, then parses the tag and the envelope. A post it cannot
  parse, or whose tag differs from the tag derived from its envelope, is answered `not-recorded`
  in the thread: nothing was claimed and nothing was fenced, so the sender treats it like a
  failure before hand-over for that attempt only (9.4), never as a refusal. Everything else goes
  to `accept --via relay:R --peer-sender ID`, which checks that `from.team` is the team mapped to
  the poster and that `to.host` is this host, and records any refusal of the message in the
  ledger as for every route (9.4). The coordinator sees only what its deliverer types: the
  receiver's own rendering (8.1), never the post.

Owner posts in that conversation keep their meaning. In the receiving coordinator's own channel
they are owner requests as today: admission splits by sender ID, and the sending bridge does not
watch that channel, so no second bridge can type the same owner message (I2). This differs from
10.6, whose relay channel is not also an owner channel, and so does the offer rule: a DM route is
offered when the sender's route data names it, the receiving declaration lists the conversation
and its peer senders (`relay_channels`, 6.4, with `"kind": "dm"`), and the last probe saw the
receiving bridge running with that conversation and its rescan on; the sending bridge need not
watch it. The sending app must be a member of the receiving coordinator's channel, which lets it
read that channel's history, the owner's requests included (it needs to, to list receipts);
question 23 asks whether that is acceptable. In a pair
conversation both bridges might watch, so owner posts there are refused, as in 10.6, until the
owner says otherwise (question 23).

**Receipts.** The receiver posts one line as a thread reply to the message, through its own
outbound helper: the message's tag followed by `receipt: accepted 13:00:12Z` (or the refusal).
The sender reads it by listing the thread with its own helper, the listing that helper already
does after an uncertain outcome (2.5), or asks over a host route. A receipt never draws a
receipt. As in 10.6, the operation UUID is derived from the ID, so the route cannot retry; an
ambiguous post is settled by a receipt, and a resend after "unknown" goes over another route to
the same contact (the receiver's ledger absorbs the post if it is read later). The helper's own
repost after an uncertain outcome cannot be switched off today (at most 86,400 s, 10.6), so the
owner may occasionally see a second copy of a post; the coordinator never sees one.

**Sparingly.** The owner reads every post in that conversation, so a chat message costs the
owner's attention where a host route costs nothing. The route therefore:

- comes last in route order (11.2) and is never first unless it is the only route;
- carries `"visible_to_owner": true` in its data, so `send` prints "this message will be posted
  where the owner reads it" when the walk reaches it, and `--no-chat` leaves it out;
- has a per-route rate limit in data (`"limits": {"per_hour": 6}` by default); over the limit it
  is a definite skip and the message waits for another route as `deferred`.

The `agent-teams` skill, written with the tool, must say the same to agents: when the only route
is chat, send only what the other coordinator needs to act on, combine several points in one
message, do not send acknowledgements, thanks or progress chatter, and prefer waiting for a host
route when nothing is urgent. Host routes (`local`, `ssh`, `command`, the hub relay) need no such
restraint. Agents never label team messages by hand; the tag is agent-teams' job. The last
sentence of the owner's answer ("If going to an underlying this with the human on the chat it
doesn't matter so much") was garbled in transcription; this rule follows the reading above, and
question 24 asks.

## 11. Route selection, topology, relaying and failure reporting

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
   - `answered: refused` (of the message, ledger-backed): definite for this attempt, and durable
     at the receiver (9.4). A policy or conflict refusal ends the message as `refused`, since
     another contact of the same team would apply the same policy. A verification refusal (the
     coordinator is not live) releases the contact: if any earlier attempt to it was ambiguous,
     the ledger entry already acts as the fence.
   - `answered: route refused` (no ledger entry: wrong host, relay not accepted, `hop-limit`,
     relay policy, spool full, `not-recorded`, `remote-too-old`): treated as `not-delivered`
     for this attempt; try the next route. It never ends the message, and after an earlier
     ambiguous attempt it does not release the contact.
   - `answered: in-custody` (a hub relay, 11.6): pinned to this contact, as for `accepted`; the
     sender then follows the custody and the final answer.
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
first: `local`, then a persistent ssh link (for a spoke, its link to the hub when the target is
the hub), then plain or on-demand ssh direct to the target, then an ssh jump through the hub
(10.3), then the hub relay (11.6), then a host-mode command route, then a chat route (10.7, and
10.6 if it is ever enabled). Session contacts come after host contacts. A chat route is never
first unless it is the only route, because the owner reads it, its latency is two bridges (and a
rescan), and its receipts are slower.

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
| `ssh` with `jump` | the receiving host's agent-teams and agentctl | as plain `ssh`; the hub is named in the route only | `receipt` over any route; `fence` | tombstone |
| hub relay (11.6) | the final host's agent-teams and agentctl, never the hub | "proved on host-b, relayed by hub-a (host-b authenticated to hub-a by its key)" | the hub's custody state, then `receipt` at the final host; `fence` at the final host only | tombstone |
| `command`, relay mode | the receiving host's agent-teams and agentctl, invoked by its bridge | "asserted by sim's bridge in #coord-relay" | receipt in thread, or over a host route; `fence` over a host route | tombstone |
| `command`, relay mode, DM (10.7) | the receiving host's agent-teams and agentctl, invoked by its bridge | "asserted by sim's bridge (app user U0PEER) in tools' conversation" | receipt in thread, or over a host route; `fence` over a host route | tombstone |
| `command`, session mode | the runtime's own `inspect`, before and after | the runtime's attribution, or "asserted" | the route's `status` operation if declared; else pinned | none (runtime has no expiry or cancel) |

### 11.5 Topology: a star of persistent links

The owner (2026-10-25): "I think probably a star topology of persistent links could work. We
don't want n squared persistent links surely. Not sure if our message later can route through a
star or any other multi hop topology." Section 19 compares how other systems do it: Gas Town,
AutoGen and MCP Agent Mail are stars around one host or server; Ruflo's federation is pairwise
with no relay; NATS uses a full mesh only inside a cluster and acyclic trees of dialled-out leaf
nodes between sites.

**Counting.** With n hosts there are n(n-1)/2 pairs. Persistent links for every pair, one
supervisor per direction where both can dial, is up to n(n-1) supervised connections: 20 for five
hosts, 90 for ten, each a unit to install, a key to enroll and a health state to watch. A star is
n-1.

**The recommendation.**

- **One hub, named in data.** `teams.json` gains a `topology` object:

  ```json
  "topology": {
    "comment": "hub-a is reachable from every host and stays up; hub-b is the standby.",
    "hubs": ["hub-a", "hub-b"],
    "standby": "on-demand"
  }
  ```

  The first hub is the hub; the rest are standbys in order. The hub is chosen for reachability
  (every spoke can open a connection to it) and uptime, not because any team lives there,
  although one may. `teams check` verifies that every spoke has a working link to the first hub
  and reports a spoke without one as `no-hub-link`. Without a `topology` object nothing changes:
  routes are as in section 10.
- **Spokes dial the hub.** Each spoke runs one persistent link to the hub (10.3's `persistent` mode,
  a user unit the owner installs), opened by the spoke, as NATS leaf nodes and AutoGen workers dial
  their hub. Over that one link the spoke **pushes** to the hub and **pulls** from it (10.3's pull,
  a long poll that returns as soon as the hub holds something for that spoke), so the link's route
  data says `"direction": "both"` (push and pull over one connection; 10.3's `push`, `pull` and
  `await-pull` remain for one-way pairs). So a spoke needs only outbound reachability to the hub,
  the hub needs no key for any spoke, and a spoke that can be reached in neither direction from
  other spokes still takes part. Where the hub can also reach a spoke, a hub-to-spoke push route may
  be added; it is not needed.
- **Spoke to hub, hub to spoke** messages are ordinary single-hop deliveries over that link. The
  hub host's own teams are reached directly.
- **Spoke to spoke**, in route order (11.2): a direct route if one is configured and up (an
  on-demand ssh connection, a command route); else an ssh **jump** through the hub (10.3), when
  the target accepts ssh from the hub; else the **hub relay** (11.6), which also serves a target
  that is down or reachable only by its own pull.

**When on-demand direct connections still make sense.** Between a pair that exchanges messages
often, or large ones (no hub hop, no hub storage); when the hub is down; for `--private`
messages, which by default do not rest on the hub (11.6, question 22); and for discovery and
checks (`probe`), which are cheap and need no custody. "On-demand" is 10.3's mode: a ControlMaster
opened at first use and closed after `ControlPersist`, so nothing lingers and no unit is
installed. On-demand routes are not counted against the star: they exist only while used.

**Hub failure.** The spokes' link supervisors mark their links `down` (10.3) within
`ping_seconds × down_after_failures`. The walk skips hub routes as definite skips and moves to
the next route: direct on-demand routes, then a standby hub. A standby is `on-demand` by default
or `persistent` when the owner asks for two links per spoke (2(n-1) links, still linear).
`on-demand` here means the spoke's link supervisor itself opens the link to the standby, with
push and pull, as soon as its link to the first hub is `down`, and closes it once the first hub
is back and the standby's spool holds nothing for that spoke. That is what lets a spoke that
only pulls receive through the standby; a standby opened only when a walk needs it would never
be pulled from. Messages already **in custody** at the failed hub do not fail over by themselves:
each is pinned to its contact (9.4, 11.6). The sender may resend the same ID directly to the
final host, or through the standby, after asking (9.3), and the final ledger absorbs the first
hub's copy if it forwards it later; moving to a different contact needs a fence at the final
host, as for any ambiguity. Nothing is promoted
automatically: which host is the hub changes only when the owner edits `teams.json`.

**Fewer keys, too.** With dedicated keys (10.3), a star needs one key per spoke on the hub (forced
command `serve --stdio --peer <spoke>`; jumps go through its `connect` verb, so no forwarding grant
is needed, 10.3), instead of one per ordered pair. A jump to a target also needs the sender's key on
the target, which is per pair again; the hub relay does not (the hub forwards with its own link or
the target's pull).

### 11.6 Relaying through the hub: store and forward, exactly once

The question is whether a message can go spoke → hub → spoke and keep section 9's guarantees.
It can, because section 9 never relied on the route: every guarantee rests on one ID fixed at
send and one durable seen-set at the **final** receiving host. A relay is one more route to the
same contact. What a hub adds is a place where a copy can wait, so the work is in saying what
custody means and how a sender learns where its copy is. The first version deliberately gives the
hub no way to prove that a copy will never be forwarded (see "Proof of non-delivery" below).

**Shape.** The sender runs `serve relay` on the hub, over its link, with the envelope unchanged:
same ID, same `to` (the final host), same sender proof. The hub's `agent-teams`:

1. checks its relay policy (a `relay.json` beside its declarations: which spokes it relays for,
   to which hosts, size and rate limits, `max_spool_bytes`), and that the envelope's `to.host`
   is a host it relays to and not itself (a message for the hub's own teams goes to `accept`).
   A failed check is a route refusal (9.4): `relay-policy`, `spool-full`;
2. refuses an envelope that already carries a relay trace (`hop-limit`, also a route refusal):
   **one relay at most**, and never hub to hub (11.7);
3. **takes custody**: creates `relay/<id>.json` in its spool by exclusive create under a short
   spool lock, recording the envelope, the authenticated spoke (from the key's forced command,
   or `asserted` under `use_user_ssh_config`) and the time, and answers `in-custody`. A second
   submission of the same ID returns the entry's current state; the same ID with a different
   body is a `conflict`;
4. forwards it as a sender would, holding that entry's own lock (never the spool lock, so new
   submissions never wait behind a slow forward) and writing each attempt record before the
   attempt: an `accept` over the hub's own route to the final host, or, for a host that only
   pulls, an offer on that host's pull (below). The final host's answer is recorded in the entry
   (`forwarded: accepted`, `forwarded: refused`, a known state). A route refusal from the final
   host (for example `accepts_relay_from` does not list this hub) is recorded as
   `forward-refused: <reason>` and the hub stops trying; the sender's walk moves to its next
   route;
5. appends one trace record to the envelope it forwards, outside the hashed body: `{"relay":
   "hub-a", "from_peer": "host-b", "peer_auth": "key|asserted", "received_at": …}`.

**Forwarding by pull.** For a final host that cannot be reached from the hub, the hub's spool is
offered on that host's pull, `serve pull --for <host>`, which 10.3 defines for a sender's outbox.
The hub's version: an entry addressed to that host is offered only under the entry's lock, the
offer is recorded before it is returned, and the puller runs `accept` locally and answers with
`serve ack ID STATE`. An offer without an ack is re-offered, with the same ID, to the same host;
its ledger absorbs a repeat. An entry is never offered to any other host.

**The final host decides everything.** `accept` on the final host verifies the coordinator,
applies its own policy and claims the ID in its own ledger exactly as for a direct route (8.3,
9.2). The hub never runs `accept` for another host, never writes another host's ledger, never
applies a team's `accepts_from`, and never renders the message. The final host additionally
checks the relay: its declaration lists the relays it accepts (`"accepts_relay_from":
["hub-a"]`), the hop came from that hub's authenticated link, and the trace has exactly one
record; a failure here is a route refusal, before any claim. It renders "proved on host-b,
relayed by hub-a", because the claim that the envelope came from host-b is the hub's (the hub
authenticated host-b's key; the final host authenticated the hub's). An optional end-to-end
signature would remove that trust (question 22).

**Two kinds of receipt.**

| Receipt | From | Says | Is it delivery? |
|---|---|---|---|
| custody | the hub | `in-custody` (since), `forwarding` (last attempt, reason), `forwarded: <final answer>`, `forward-refused: <reason>`, `expired-in-custody` | no; only `forwarded: accepted` or later tells the sender the final host has it |
| end to end | the final host's ledger | the states of 8.4 | yes |

The sender records `in-custody` (8.4), which is **not delivered** and exits 75 (not settled).
`pump` then asks the hub (`serve relay-status ID`, a pure read), which reports the custody state
and caches the final host's answer once it has one, and may ask the final host directly over any
route. As with SMTP's accepted responsibility, custody is a promise to try or to report, never a
claim of delivery. `forward-refused` is a route refusal: the walk moves to the sender's next
route for the same contact.

**Ambiguity across hops.** The rules of 9.3 and 9.4 apply unchanged, with the final host as the
contact and the hub as one of its routes:

- **An ambiguous submission to the hub** (the link dropped after the envelope was written) is
  resolved by asking the hub `relay-status`, then the final host `receipt`. When the hub has no
  custody record of it, the sender may resubmit with the same ID; when the final host reports it
  unknown, the sender may resend with the same ID over another route to the same contact (9.3).
- **Copies at the hub and a direct resend are both allowed, for the same contact.** A copy in
  custody and a resend over a direct route, or a second hub, all carry one ID to one final
  ledger, which claims the first to arrive and returns that state to the rest. That is the same
  reasoning as 9.3, and it is what lets a sender go around a hub that is down while it holds a
  message. To keep the number of waiting copies small, `pump` submits to a standby hub only while
  the first hub is known down.
- **Proof of non-delivery comes only from the final host.** A message in custody, or whose
  submission to a hub was ambiguous, may still be forwarded, so the contact is **pinned** (9.4)
  until the final host answers. Moving to a different contact needs a fence at the final host
  (9.4), which also bars every copy still in a spool: it is refused when it arrives
  (`forwarded: refused (fenced)`). Where the final host cannot be reached, the message stays
  pinned until `not_after` and then ends as below. A hub-side withdrawal that would release the
  contact without the final host is not in the first version: to be sound it needs a durable
  "sending" mark taken under the entry lock before every forward attempt, and a spool identity
  that a backup or snapshot restore cannot copy, or a restored spool would forward a withdrawn
  message. Second contacts are rare (a session fallback), so this costs little.

**What "delivered" means when the hub holds it.** Nothing new: a message is delivered when the
final host's ledger says so. A message held by a hub until `not_after` passes can no longer be
delivered, because the final `accept` refuses an expired envelope. If any of the hub's forward
attempts, or any of the sender's own attempts to that contact, was ambiguous, the sender ends
`ambiguous-expired` (question 19's `resolve` settles it); if none was, the hub reports
`expired-in-custody` and the sender ends `expired`. The hub's custody entries follow 9.2's
retention rule (kept until 30 days past the envelope's own `not_after`). Once the final answer is
recorded, the hub drops the body and keeps the metadata.

**What the hub can read.** A store-and-forward hub holds the whole envelope, body included, in
its spool (0700, under the owner's account, the same account as every other host here). A jump
(10.3) holds nothing and reads nothing. Every route whose data stores or shows the body to
anything other than the sending and receiving agent-teams declares `"exposes_body": true`: a
hub relay route (`via_hub` implies it), every chat route, and any command route that says so.
The walk skips such routes for `--private` messages (11.1), so by default `--private` messages
take a jump or a direct route; whether that default is right is question 22. Encrypting bodies to
the final host, so that a hub holds only ciphertext, is not proposed now.

**The hub's own authentication.** A spoke's link to the hub uses a dedicated key with the forced
command `agent-teams serve --stdio --peer <spoke>` (10.3), which binds every custody record to
the spoke that handed it over; or the owner's ssh configuration (`use_user_ssh_config`, allowed
permanently since 2026-10-25), in which case the hub records the peer as `asserted` and the final
host renders it so. The hub forwards over its own route to the final host (its own key there) or
by the final host's pull over that host's link to the hub (the puller's key on the hub), so the
final host always authenticates the hub, never the original spoke.

**Verbs and commands.** `serve` gains `relay` (takes custody) and `relay-status` (a pure read).
On the hub, `agent-teams relay show [ID]` lists the spool and `agent-teams relay pump` forwards
(a timer or the link supervisor runs it). The sender's route is an ssh (or command) route with
`"via_hub": true`, listed in each target's `routes` like any other.

### 11.7 Why one relay, and not a general mesh

A mesh in which any host forwards to any other, over several hops, would keep the exactly-once
guarantee (the final ledger still decides) but is not worth building here:

- **It buys nothing at this scale.** With a hub every spoke reaches, every pair has a path of at
  most two hops, and a standby hub covers the hub's failure. A third hop helps only a host that
  cannot reach any hub, which is a configuration to fix, not to route around.
- **Every hop is another place a copy can wait.** The custody and pinning rules above
  would apply at each hop, and a sender would need every hop's answer to prove non-delivery
  before moving on; with two hops it needs one.
- **Multi-hop needs a routing protocol.** Paths would have to be computed (or written by hand,
  as UUCP bang paths were), kept current and protected against loops; netnews uses a `Path:`
  history and SMTP counts `Received:` fields up to about a hundred (19). A hard one-relay rule,
  as NATS's one-hop forwarding within a cluster, makes loops impossible instead of detected.
- **Debugging stays local.** A message's whereabouts is one of: the sender's outbox, one hub's
  spool, the final ledger.

If the owner's hosts ever outgrow one star (sites with their own hubs), the extension is hubs
that peer with each other as gateways do in NATS, still with a fixed, short hop limit; it is not
designed here.

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
`agentctl drain --only-prefix tm-` is question 21 (left open when question 13 was decided; the
default is `agent-teams drain`); either way it is the receiving
host's own process (principle 1).

### 12.4 Older drainers must not see what they cannot honour

Old binaries ignore unknown JSON fields, so a marker inside an item refuses nothing: a pinned
bridge, watcher, helper or rollback binary would type an expired `hold`. Items with team fields
therefore go to a new lane, `queue/inbox.v2/`, which binaries before phase 2 never list. New
drainers merge `inbox/` and `inbox.v2/` in enqueue order. Until a queue's drainers are new, its
`inbox.v2/` items are typed only by the bounded team drain, and `accept` says so in the receipt
when it finds a running drainer whose binary predates the lane (`agentctl capabilities` on that
path). Every Rust consumer that hardcodes the four phases must learn `inbox.v2/`, `cancel/` and
`cancelled/`: agentctl's enqueue duplicate check and `message_state` (`agent.rs:1422`, `1839`),
and the helper in 2.3 (or it becomes the wrapper of 15.1). The Python edition of agentctl gets no
port of these features (question 11, decided 2026-10-25): it gets a **guard** instead. When a queue has an
`inbox.v2/`, `cancel/` or `cancelled/` directory, or an item carries `not_after`, the Python
edition refuses to drain, deliver or re-enqueue in that queue, and says to use the Rust edition.
The guard has to cover the Python chat runtime's re-enqueue of a `received` item it cannot find
(`py/agentctl/chat_runtime.py:784-797`), which would otherwise re-enqueue an item that had moved
to a new lane, a duplicate. The cross-edition differential checks exactly that refusal. Local
`agentctl send --expires/--topic/--supersedes` is enabled only after that inventory covers every
writer.

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
| `fqdn`, `ssh` | an `ssh` route in `routes.d` (with `use_user_ssh_config: true`, which may stay; `ssh enroll` is optional, 10.3); a `command` route by hand where ssh is refused |
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
agent-teams send TEAM[@HOST] (--file F | --text T) [--key K | --again] [--topic T] [--supersedes K]... [--ttl D | --expires TIME]
    [--kind K] [--private] [--no-chat] [--mirror-to-chat ROUTE] [--from-service NAME] [--allow-undeclared] [--allow-unexpiring] [--wait S] [--json]
agent-teams reply ID --file F [--key K]
agent-teams retract ID
agent-teams status ID | outbox | pump [--retry-refused] | pull --for HOST
agent-teams relay-in --route R --peer-sender ID              # (target) a chat route's one command (DM or channel): envelopes and receipts
agent-teams seen ID                                           # local ledger: has this ID been delivered here?
agent-teams redeliver ID                                      # human only: show a delivered message again, marked as such
agent-teams resolve ID (--fence | --mark-delivered)           # human only, on the receiving host: settle an ambiguous-expired message
agent-teams hook                                              # harness hook for the hook-inbox deliverer
agent-teams drain [--deadline S] [--lock-timeout S] [--max-items N]   # (target) types tm-* only

agent-teams ssh enroll --peer HOST [--jump-to TARGET]...      # make a key; print the authorized_keys line (a jump-only grant per TARGET on a hub)
agent-teams relay show [ID] | pump                            # (hub) list the relay spool; forward what it holds
agent-teams link run ROUTE | up ROUTE | down ROUTE | status [ROUTE]

agent-teams serve --stdio --peer HOST                         # (target) forced command: accept, receipt, fence, probe, ping, pull, ack, relay, relay-status, connect
agent-teams accept --protocol 1 [--team T] [--via relay:ROUTE --peer-sender ID] (--envelope - | --envelope-base64 B)
agent-teams receipt ID | fence ID --not-after TIME | probe --json              # (target) receipt and probe are pure reads; fence takes the ledger lock

agentctl send NAME --file F [--expires TIME] [--topic T] [--supersedes ID]   # generic queue features, after 12.4
```

Global: `--teams FILE` (default `$AGENT_TEAMS_FILE`, then
`$XDG_CONFIG_HOME/agent-teams/teams.json`) and `--routes DIR` (repeatable; added after `routes.d/` and
`$AGENT_TEAMS_ROUTES_PATH`, 6.3).

Exit status extends `agentctl send`'s 0, 75 and 76 with 65 for terminal non-delivery, which
`agentctl send` does not need:

| Exit | Meaning | Safe to re-run the same command? |
|---|---|---|
| 0 | `delivered`, or `journaled` for a session contact | yes; it reports the same state |
| 75 | not settled: `accepted`, `deferred`, `awaiting-pull`, `in-custody`, or `--wait` ended first | yes; the ID makes it a retry |
| 76 | uncertain: `ambiguous`, `ambiguous-expired`, `possibly_submitted`, `lost` | yes; it asks, it never resends blind |
| 65 | terminal and undelivered: `expired`, `superseded`, `retracted`, `refused` (including every contact refused or fenced), `journaled-halted`, retract too late | yes; nothing new is sent |
| 2 | usage or configuration | n/a |

## 17. Phased plan

1. **P0: extract the shared library.** Queue format, enqueue duplicate check, harness identity
   and ancestry into `agent-queue`, with agentctl switched to it and no behaviour change; the
   queue format documented with the crate. (Coordinated with the suite-factoring lane's
   audit.)
2. **P0b: the coordinator designation in agentctl** (revision 3; decided, question 27). An
   agentctl feature in its own right, useful without team messaging, closing the gap in 2.1:
   - **One designation per registry**, `REGISTRY/coordinator.json` (`{"schema":
     "agentctl-coordinator/v1", "name", "token", "set_at", "set_by"}`), set with `agentctl
     coordinator set NAME`, read with `agentctl coordinator show [--json]`, removed with
     `coordinator clear`. `adopt --as-coordinator` and `attach-self --as-coordinator` set it in
     the same step; `rename` carries it to the new name; `stop` of the designated agent leaves
     it naming an archived record, which `show` and `status` report as `coordinator-stopped`.
   - **Its consumers read it**: `restore` (whose `restore.json` `coordinator` becomes optional
     and must agree with it when present), `watch-agents` (`--coordinator` defaults to it),
     `chat run` (its target agent, when `chat.json` names none, and a warning when it names
     another), and wrkslots, which can take the designated record's pinned harness process ID
     instead of a caller-supplied `--coordinator-pid`.
   - **Edges, optional but recorded by default.** Every record gets two separate fields, written
     unless the caller opts out (`agentctl start --no-edges`, or `"record_edges": false` in a
     launch profile). Each names an agent by `{"agent", "registry", "token", "proof":
     "harness-ancestry"}`, found by the ancestry walk of 8.2, and is absent when the caller
     cannot be proved to be a registered agent (a human at a shell, a timer):
     - `created_by`, **provenance**: who ran `agentctl start`. Set once, never changed, possibly
       absent. `status` and `list --tree` show it.
     - `managed_by`, **the current controller**: set at `start` to the same agent, **replaced**
       when another coordinator runs `agentctl adopt` on the record (the normal hand-over: the
       owner often tells a new coordinator to adopt agents in tabs), and clearable with
       `agentctl manage --clear NAME`.
     Decisions about control follow `managed_by` or the registry's coordinator designation,
     never `created_by`: who may send a hold or other directive to an agent, which coordinator
     wrkslots hints as a slot's owner, and whom `watch-agents` notifies about an agent (its
     `managed_by`, else the designated coordinator). `created_by` is history only.
   - agent-teams reads the designation (6.4's `"designation": "registry"`) and never writes it.
3. **P1: read-only model.** `teams.json` and `routes.d` parsers with schema and comment handling,
   `teams show`, `probe`, `check`, `discover` (proposal only), `import`. Surfaces I1, I2 and I4
   with no change to delivery.
4. **P2: queue lanes and cancellation** in the library and the Rust agentctl: `inbox.v2/`,
   `cancel/`, `cancelled/`, `not_after`, the pre-paste re-check, bounded drain flags, the 12.4
   inventory; in the Python edition only the guard that refuses queues using the new lanes
   (question 11), with cross-edition differential cases for that refusal.
5. **P3: exactly-once core and local delivery.** Declarations, ledger with claim, fence and
   recovery; `accept`, `receipt`, `fence`, `serve`; deliverers `agentctl-queue` and
   `hook-inbox`; `send`, outbox, the walk, `status`, `pump`, `retract`; sender proof
   (coordinator, service) with a test per mode; the `local` route. **The model-based harness of
   9.7 lands in this phase, before any network route.**
6. **P4: network routes and the star.** The `ssh` driver with `use_user_ssh_config` first (it
   needs no trial), then `enroll`, the restricted forced command and its own known-hosts; links
   (`on-demand`, `persistent`) and pull; the `topology` object, spoke links to the hub and
   `no-hub-link` checks; the `jump` option and jump-only grants; the `command` driver in host and
   session modes; route health; the harness extended with each driver's fault model. **Live
   trials** (9.7) of dedicated keys, persistent and pull links, and jumps, each recorded before
   the owner lists it in `trials`.
7. **P5: replies, policy, migration, chat DM.** `reply`, `pull`, `accepts_from` with kinds,
   operator grants, limits, retention, `resolve`; the helper in 2.3 as a wrapper; the bridge's
   generic coordinator-peer admission class and the chat DM route (10.7), with its rate limit
   and `--no-chat`; the upstream bot-author filter's exception for listed peers, if the owner of
   that client agrees (otherwise the rescan carries DMs); the `agent-teams` skill with the
   sparing-use rule; the shared-pane range check in `chat run`; a live trial of the DM route.
8. **P6: the hub relay** (11.6). The spool with custody and per-entry locks; `serve relay`,
   `relay-status`; forwarding by push and by the target's pull; the final
   host's `accepts_relay_from` and trace check; `in-custody` on the sender. The harness's relay
   faults (9.7) land first, then a live trial with the hub stopped while holding a message.

P1 and P2 are independent of each other once P0 is done. P0b needs nothing else and is needed
by P3, whose declarations read the designation. Fixing I3 for ordinary local sends waits
for P2 and its inventory. P6 needs P4's links and pull, not P5. The shared coordinator channel of
10.6 is in no phase until the owner has reviewed it.

## 18. Questions for the owner

Numbers 1-11 keep revision 1's numbering; 12 onward are new.

**Resolved**

- **Format** (new): JSON, with explicit `comment` and `notes` fields and a versioned `schema`
  (6.1). Owner, 2026-10-24.
- **8. Workers as senders:** only coordinators, plus registered services, which this design keeps
  explicitly for operational alerts (8.2). Workers neither send nor receive. Owner, 2026-10-24.
- **11. Editions.** Asked "Wait I thought we were staying. All rust or rust by default. Why do
  we need to write python for this?", and shown the proposal below, the owner approved it: "Yes
  flip repo default to rust. Put only a refusal in python as you said. We can catch python up
  later when things stabilize." (2026-10-25). `agent-teams` is Rust only. The Python edition of
  agentctl gets no port of the queue features, only a guard that refuses to drain, deliver or
  re-enqueue in a queue using the new lanes (`inbox.v2/`, `cancel/`, `not_after`) and points to
  the Rust edition; the cross-edition differential checks just that refusal (12.4, 17). Catching
  the Python edition up is later work. The repository-wide switch of the engine default to Rust
  is a separate lane (`engine-rust-default`), not part of this design.
- **27. The coordinator as an agentctl concept.** On the proposal of a registry-level coordinator
  designation plus edges between agents (P0b, 17), the owner: "I think we should keep it edges
  optional but maybe default. I often tell a new coordinator to adopt agents in tabs. So while
  it's good to know who created a subagent it doesn't determine who controls it in the future."
  (2026-10-25). Edges are optional but recorded by default, in two fields: `created_by`
  (provenance, set once) and `managed_by` (the current controller, replaced by `adopt`).
  Control follows `managed_by` or the registry's designation, never `created_by`. The
  registry-level designation stays as proposed.
- **12, second half. The owner's own ssh configuration.** Asked "May routes use your own ssh
  config during migration, and should that expire?", the owner answered "Sure and no. But good
  to test any other mechanism we propose." (2026-10-25). `use_user_ssh_config` is allowed and
  never expires: a permanent mode, not a migration step (10.3). Every other mechanism is relied on
  only after tests and a recorded live trial (9.7). The first half (dedicated keys) stays open.
- **13. Where it lives.** "Yes and let's just name it agent-teams for now." (2026-10-25): a
  separate `agent-teams` binary on an extracted `agent-queue` crate (4). Its sub-question, where
  bounded local typing lives, is open as question 21.
- **15, partly. Chat between coordinators.** "Not yet. I'll review the design if it's in there.
  But for starters I want just dms to other coordinators with clear tagging. [Coord1 <host> ->
  coord 2 <host>]. The agents seem to usually want to label their inter agent messages but are
  inconsistent in how they do it. Our agent team skill needs to emphasize to be use it sparingly
  if using the chat based channel. If going to an underlying this with the human on the chat it
  doesn't matter so much." (2026-10-25; host names redacted). No shared coordinator channel for
  now: 10.6 is kept for review and is in no phase. The first chat route is a direct message to
  the other coordinator's conversation (10.7). Every route carries one fixed team tag (8.1). The
  `agent-teams` skill tells agents to use the chat route sparingly; host routes need no such
  restraint (10.7). Still open: one bridge app per team for the shared channel, if it comes back,
  and the questions the DM raises (23, 24, 26).
- **Chat-DM route (P5) stays off; hub-and-spoke relay (P6) shelved.** Owner, 2026-10-25:
  "chat-DM route stays off for now, and hub-and-spoke relay is shelved (only two teams matter
  now; direct routes suffice)". Neither route is implemented; 10.6/10.7 and 11.6 remain design
  text for review. If either returns, the `agent-teams` skill's restraint rule applies to the
  chat route (sparingly, tagged, short), and relay acceptance stays gated by
  `accepts_relay_from`.
- **17. Private route files.** "Company internal I don't care where exactly. Git ignored
  files." A more durable copy may live in a private repository the owner keeps (2026-10-25;
  quote shortened to leave out a private path). Private routes go in git-ignored
  `*.private.json` files, found through `routes.d/`, `AGENT_TEAMS_ROUTES_PATH` and `--routes`;
  the public repository ships placeholders only (6.3).
- **18. Hook-only teams.** "18 yes." (2026-10-25): the `hook-inbox` deliverer is accepted for
  teams without agentctl, with its limit that it never wakes an idle agent (4.4).
- **19. Unresolvable ambiguity.** "uh sure whatever you recommend. Don't care." (2026-10-25):
  the recommendation stands. A human-only `agent-teams resolve ID (--fence | --mark-delivered)`,
  run on the receiving host when it is reachable again, settles an `ambiguous-expired` message
  (16).
- **20. Test budget.** "I think it makes sense to repeat the split of regression tests plus
  optional or nightly soak" (2026-10-25): a bounded harness pass in the regular gate, under a
  minute, and a soak as an optional target that the nightly workflow runs, with failing seeds
  kept as artifacts and added to the gate's fixed list (9.7).

**Open**

1. **Where do `teams.json` and `routes.d` live across hosts?** Recommendation: a copy of
   `teams.json` and the public routes on every host from a dotfiles repository; private route
   files as decided under question 17. Senders work from their local copy; staleness is handled
   by the checks.
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
9. **Retention.** Ledger, fence and custody entries kept until 30 days past each entry's own
   `not_after` (9.2), so pruning can never reopen an ID? And should this work also prune
   agentctl's `processed/`?
10. **Clock skew.** A 2 s cross-host tolerance for topic order, or require one sending host per
    topic?
12. **Dedicated keys** (first half). A dedicated key per host pair (per spoke, in the star),
    restricted by a forced command to the protocol, installed by you by hand? Recommendation:
    keep them **optional**. `use_user_ssh_config` is a permanent supported mode (resolved above),
    and dedicated keys are offered where stronger attribution or least access is wanted, once
    their live trial is recorded.
14. **Topology.** Your answer pointed at a star of persistent links and asked whether messages can
    route through it. Recommendation (11.5-11.7): one hub named in `teams.json`; every spoke
    dials one persistent link to it (n-1 links; one-way-reachable spokes work by pull over their
    own link); spoke to spoke goes direct when an on-demand route exists, else by an ssh jump
    through the hub, else by a store-and-forward relay at the hub that keeps the exactly-once
    rules with the final host as the only authority; one relay at most, no general mesh. Do you
    accept the star with one relay, and the hub relay as phase P6?
16. **Bodies in a shared channel** (deferred with 10.6). If the shared channel comes back, may
    non-private bodies appear in it, or should it carry only the ID, with the body fetched over a
    host route? (In the DM route, bodies are visible to you by design; `--private` messages never
    take a chat route.)
21. **Bounded local typing** (from question 13). `agent-teams drain` (the default here, calling
    the shared crate) or `agentctl drain --only-prefix tm-` (12.3)?
22. **What the hub may hold.** The hub relay stores whole envelopes, bodies included, until the
    final answer (then metadata only). Recommendation: accept that for non-private messages (the
    hub runs under your account, as every host does); keep `--private` messages off the hub (a
    jump or a direct route carries them); and add, later and optionally, an end-to-end signature
    by the origin host (`ssh-keygen -Y sign`, verified at the final host) so the final host need
    not trust the hub's claim of origin. Encrypting bodies for the final host is not proposed.
23. **The DM route's conversation.** The DM route posts into the receiving coordinator's own
    bridge channel, so the sending team's bridge app must be invited there, and can then read that
    channel's history, your requests to that coordinator included (10.7). Is that acceptable, or
    should every pair get a conversation of its own (you and the two apps)? In a pair
    conversation both bridges may watch, so your posts there are refused by default (to rule out
    two bridges typing one message, I2): should they instead be requests to one coordinator, and
    which?
24. **The last sentence of your question-15 answer** ("If going to an underlying this with the
    human on the chat it doesn't matter so much") was garbled in transcription. The design reads
    it as: use the chat route sparingly because you read it; host routes need no such restraint.
    Is that the meaning, or did you mean that messages going into a conversation you are already
    in need less restraint?
25. **Which host is the hub, and is the standby persistent?** Recommendation: the host every other
    host can reach and that stays up; one standby, `on-demand` (2(n-1) links only if you want the
    standby kept open).
26. **The bot-author filter.** The provider client's tail drops bot-authored posts, so DMs arrive
    only through the bridge's rescan (up to about 90 s late). Ask that client's maintainers for an
    exception for listed peer IDs, or accept the rescan latency for a last-resort route
    (recommended to start)?

## 19. Related work

### 19.1 Cross-host agent messaging and topology (revision 3)

The owner asked how Gas Town and others manage cross-host messaging and which pairs get standing
connections. The full comparison, with sources, commits read, and what was read versus recalled,
is the draft [`RELATED_WORK.md`](2026-10-24-agentctl-team-messaging/RELATED_WORK.md) for the
future tool. In short:

| System | Cross-host shape | Standing connections | Relay | Duplicate control |
|---|---|---|---|---|
| Gas Town | agents live on one town host; remote workers call it through an mTLS proxy that injects their certificate identity; towns share state through a Dolt hub, with no town-to-town messages yet | star | none for messages | idempotent two-phase acks; sends not idempotent |
| MCP Agent Mail | one HTTP server every agent calls | star | none | none found for sends |
| Ruflo (Claude Flow) federation | peer-to-peer WebSocket sessions; "no DERP-equivalent relay" | pairwise | none | new envelope ID per send |
| AutoGen distributed runtime | workers dial a host servicer over gRPC | star | the host, in memory only | none; cancelled on disconnect |
| A2A | direct HTTPS calls to an advertised endpoint | none | none | `messageId` dedupe optional |
| NATS | full mesh in a cluster with a one-hop rule; dialled-out leaf nodes in acyclic trees | mesh, then tree | one hop | `Nats-Msg-Id` window |
| Matrix | servers push directly, per-pair `txnId`, no new txn until the last is acknowledged | none | none | per-hop txn + end-to-end event ID |
| ssh `ProxyJump`, Tailscale DERP | ciphertext through a jump host or relay | optional | transport only | n/a |
| SMTP, UUCP, netnews | store and forward with trace headers | none | many hops | netnews `Message-ID` history; mail loop counts |

What the design takes: the star whose spokes dial out (AutoGen, Gas Town's proxy, NATS leaf nodes);
a hard one-relay limit instead of loop detection (NATS); custody as a promise distinct from
delivery, and a trace of hops (SMTP); an end-to-end ID checked against a durable history at the
receiver only (netnews, JetStream); per-hop answers before moving to another contact (Matrix); a
transport-only relay where it suffices (`ProxyJump`, DERP); and credentials that name the peer (Gas
Town's proxy certificates, forced-command keys). What none of them has is the terminal composer at
the end, typed into by one local program only when the agent is idle.

### 19.2 Designs behind sections 8-12

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
program may type into, and only when the agent behind it is idle. The `RELATED_WORK.md` the tool
needs starts from the draft named in 19.1.

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

**Revision 3** (2026-10-25, owner answers, then one adversarial review):

- Resolved with the owner's words: question 12's second half (`use_user_ssh_config` allowed and
  never expiring; every other mechanism needs tests and a recorded live trial, 9.7, 10.3), 13
  (`agent-teams`, a separate binary on `agent-queue`), 15 in part (no shared channel yet; a
  direct-message chat route; one tag; sparing use), 17 (git-ignored `*.private.json` route files
  on a search path, 6.3), 18 (`hook-inbox` accepted), 19 (`resolve` as recommended) and 20 (gate
  pass plus nightly soak, 9.7) and 11 (Rust only; the Python edition only refuses queues that
  use the new lanes, 12.4, 17).
- Topology (11.5): a star of persistent links named in `teams.json`'s `topology`, dialled by the
  spokes, with pull for the reverse direction, on-demand direct routes where they pay, a standby
  hub, and no automatic failover of messages in custody.
- Relay (11.6, 11.7): the `jump` option for ssh routes (`ProxyJump` through the hub, end to end);
  a store-and-forward hub relay with custody receipts distinct from end-to-end receipts, the
  final host as the only authority and the only source of proof of non-delivery, one relay at
  most, and a statement of what the hub can read; why a general mesh is not worth it. New sender
  state `in-custody` (8.4, 16); phase P6 (17).
- The review of revision 3 (13 findings; code and source citations confirmed) changed: chat
  posts that cannot be parsed are answered `not-recorded`, never a fence, after the provider's
  escaping and the rescan's note are undone (10.6, 10.7); refusals of the route (wrong host,
  relay not accepted, hub policy, `hop-limit`) are separated from ledger-backed refusals of the
  message and never end it (9.2, 9.4, 11.1); the hub's withdrawal and spool epochs were dropped
  from the first version, so custody pins the contact until the final host answers, and the
  forward holds a per-entry lock (11.6); the jump runs through the hub's forced command
  (`serve connect`), with no forwarding grant (10.3); repeating a delivered message without
  `--key` is refused instead of silently succeeding (8.1); fences carry a `not_after` for
  retention, and question 9 matches 9.2; the standby hub and the link's push-and-pull direction
  are specified (11.5); `pull --for` takes a host; the rescan's real limits bound the DM route's
  TTL (10.7); the tag grammar is case-sensitive and every body line starting with `[` is
  indented (8.1); the hook deliverer re-checks expiry and tombstones before printing (9.5); a
  question asks whether another team's bridge app may join a coordinator's channel (23).
- One machine-parsable team tag, `[sender host -> team host id]`, defined once and written by
  agent-teams on every route and in the rendered paste (8.1).
- The chat DM route (10.7), with the two bridge blockers (the upstream bot-author filter, so only
  the rescan sees peer posts; admission limited to `allowed_senders`) and the coordinator-peer
  admission class that fixes them; the shared channel of 10.6 deferred pending the owner's
  review.
- Related work (19.1 and a draft `RELATED_WORK.md`): Gas Town, MCP Agent Mail, Ruflo, AutoGen,
  A2A, NATS, Mosquitto, Matrix, Tailscale DERP, OpenSSH, and SMTP, UUCP and netnews (the last
  three recalled, because every fetch of the RFCs was refused).
- The coordinator designation as an agentctl feature independent of teams (P0b): the gap stated
  in 2.1, a registry-level designation its own consumers read, and edges recorded by default
  with an opt-out, split into `created_by` (provenance) and `managed_by` (control, replaced by
  `adopt`), as the owner decided (17, question 27).
- Model-harness relay faults and invariants (9.7); new questions 21-26 (18).

# agent-teams user guide

`agent-teams` carries a message from one team's coordinator to another's,
across hosts, exactly once. A **team** is a coordinator plus the workers it
manages, on one host. Control inside a team is a tree: the registry's
designated coordinator at the root, lieutenants below it. Cross-team sending
stays with the root coordinator and registered services; a lieutenant or
worker that needs another team asks its coordinator. Messages are addressed
to a team and delivered to its designated coordinator, never to whoever
happened to start the receiving process.

## Files

All files are JSON with a versioned `schema` (`agent-teams-teams/v1`,
`agent-teams-routes/v1`, `agent-teams-declaration/v1`,
`agent-teams-message/v1`). Any object may carry `comment` (string or list)
and `notes` (object); both are ignored and preserved by machine rewrites.
Unknown keys are warnings, and errors under `check --strict`. Files writable
by group or others are refused.

- **teams.json** — `$XDG_CONFIG_HOME/agent-teams/teams.json` (or
  `$AGENT_TEAMS_FILE`, or `--teams`). A sparse routing hint: hosts with
  ordered route names and registered services; teams with one entry per host
  and ordered contacts (`host`, or `session` for a runtime session); an
  optional `topology` object naming star hubs. A team, or one of its
  entries, may also carry its own ordered `routes` list (see "Per-team
  routes" under Routes). Stale data costs a warning,
  not a misdelivery: the receiving host re-checks everything.
- **routes.d/*.json** — route definitions by name. Private definitions (for
  company-internal channels) go in git-ignored `*.private.json` files, found
  in `routes.d`, in `$AGENT_TEAMS_ROUTES_PATH` directories, and in `--routes`
  directories, one namespace. `*.example.json` files are placeholders and are
  never loaded as live routes.
- **declaration.json** — per team per host, written by `init` under
  `$XDG_STATE_HOME/agent-teams/teams.d/<team>/`. Names the deliverer, how
  the coordinator is designated (the agentctl registry's designation, or a
  pinned harness identity from `--coordinator-self`), `accepts_from` policy,
  limits, and relay hubs accepted.
- **State** — `outbox/<id>.json` (sender), `teams.d/<team>/ledger/<id>.json`
  (receiver; the durable seen-set), `team-log.jsonl` (no bodies), `seq.json`,
  `links/<route>.json` (link health). Override with `$AGENT_TEAMS_STATE_DIR`
  or `--state-dir`; the host key with `$AGENT_TEAMS_HOST`.

Paths default under `0700` directories; create the state directory so.

**Shared configuration, private state.** `teams.json` and public route
definitions may be shared or even synced between hosts (a route list can
name `local` first for every host; it is skipped for other hosts). Host
state — declarations, ledger, outbox, link state, enrolment keys — must
never be shared or synced: a synced ledger would break exactly-once.
Some setups sync all of `~/.config` between a user's hosts; keep state
under the default `~/.local/state/agent-teams` there, never point
`$AGENT_TEAMS_STATE_DIR` into a synced tree, and install the binary
per host outside synced directories. `check` warns when the state
directory sits under a config tree.

## Commands

### Read-only model

- `show [TEAM] [--json]` — teams, hosts, routes, and freshness (`stale?`
  past 7 days unreviewed).
- `check [TEAM[@HOST]]... [--strict] [--json]` — verifies that local
  entries have a live declaration and coordinator, that route names exist,
  and that a spoke has a route to the first hub (`no-hub-link`). Exit 0 when
  every selected entry has at least one verified contact.
- `probe [--json]` — what this host's own tools say: declarations,
  coordinator liveness, findings. Peers run it through routes; it never
  reads another host's panes.
- `import --coordinators-file FILE --team NAME` — converts a per-host
  coordinators file into proposed `teams.json` and routes. Prints only.
- `coordinators [--json]` — who coordinates what on this host, from the
  per-host coordinator index `agentctl` maintains (below), plus the teams
  file's entries on other hosts with their routes. This is the answer to
  "which team-lead coordinators are active here, in what directories,
  and how do I message one": each row names the team, the coordinator,
  live or stale, the harness working directory, and the repository
  origin when one was found.
- `join TEAM --registry R [--accepts-from SENDER]...` — one-step team
  setup on this host: writes the declaration like `init`, then adds this
  host's entry to teams.json when it is not there yet.
- `relay setup|status|announce|teams|confirm|ban|alarms` and `relay-in` —
  the chat relay: onboarding, discovery, stranger alarms, and the bridge's
  inbound hand-off (see "Chat relay" below). A **team** is one host's
  agents under one coordinator (`TEAM@HOST`); a **fleet** is all the teams
  across hosts on the same repository (the name before the `@`). Relay
  mappings name a fleet plus a team (`FLEET@HOST`).

### The host coordinator index

Designating a coordinator (`agentctl coordinator set`, or an
adopt/attach with `--as-coordinator`) registers it in a per-host index:
one file per registry under `$XDG_STATE_HOME/agentctl/coordinators`
(else `~/.local/state/agentctl/coordinators`; `$AGENTCTL_COORDINATOR_INDEX_DIR`
overrides). An entry holds the registry path, coordinator name and
token, harness working directory, harness identity, git origin URLs of
the harness's checkout(s) found by reading `.git` configs, and the
agent-teams team whose declaration names that registry when one does.
No secrets; written atomically; `coordinator clear` removes the entry.
It lives in host state, never in synced config. The registry's own
`coordinator.json` remains the authority - the index is a discovery
cache, so readers treat dead harnesses and changed boots as stale rows,
and `send TEAM` can fall back to it: a team missing from teams.json
resolves through a live index entry on this host, addressed at its
designated coordinator over the local route.

**Same-host trust.** A sender with no explicit `accepts_from` rule is
accepted by default when it is another team's designated coordinator
on this same host, ancestry-proved, sending from its own host. Same
user, same host: such a coordinator could already act in this harness
directly, and the ledger claim, size, TTL and rate limits all still
apply. An explicit rule for that sender always wins - including one
that lists other kinds only - and cross-host senders always need an
explicit rule.

### Messaging

- `init TEAM (--registry R | --coordinator-self [--coordinator-pid PID])
  [--deliver agentctl-queue|hook-inbox|command] [--argv CMD...]
  [--accepts-from SENDER]... [--accepts-relay-from HUB]...` — declares this
  host's team. Re-running `--coordinator-self` from a new session refreshes
  a pinned identity; existing `comment`, `notes`, `may_send_to` and (when
  `--accepts-from` is absent) policy are preserved. For a `command`
  deliverer, everything after `--argv` is the command.
- `send TEAM[@HOST] (--file FILE | --text TEXT) [--key KEY | --again]
  [--topic SLUG] [--supersedes KEY]... [--ttl DUR | --expires TIME]
  [--kind info|request|hold|resume|reply] [--private]
  [--from-service NAME] [--from-team TEAM] [--json]`
  - `--file -` reads the body from stdin. DUR is seconds or `s/m/h/d`;
    TIME is RFC 3339 UTC. Default TTL: the receiving team's
    `default_ttl_seconds`, else 2 hours; never beyond its `max_ttl_seconds`
    (default 7 days). There is no `--ttl none`. Replies (`reply`, or
    `--kind reply`) get at least 24 hours by default instead: replies
    were the messages that expired while a direct route was down, and a
    relayed reply must survive the bridges' rescan window. An explicit
    `--ttl` or `--expires` always wins. The receiver refuses (it does
    not clamp) an envelope beyond its `max_ttl_seconds`, so the sender
    caps the reply default at the destination's max when that
    declaration is readable locally; a remote destination must allow
    24 hours (`max_ttl_seconds` >= 86400 — the declaration default is
    7 days, so only a team that lowered it is affected).
  - The ID is fixed at the outbox write and carried on every route. Without
    `--key` the key derives from (kind, topic, body), so re-running is a
    retry. Same key with a different body is refused as a conflict; a repeat
    of a settled message under a derived key is refused (`refused: repeat`,
    exit 65) — pass `--key` or `--again`.
  - `--topic` makes the newest message from one sender supersede older
    untyped ones on that topic. `--supersedes KEY` cancels one earlier
    message. Both act at the receiver, where delivery happens.
  - `--private` keeps the body off routes that expose it.
  - `--from-service NAME` needs NAME registered for this host with a systemd
    unit and this process inside that unit; it is refused when an agent
    harness is in the ancestry.
  - Proof is taken once, at the outbox write, in the sender's process tree:
    the ancestry must contain the designated coordinator's harness process.
- `reply ID ...` — replies to the sender's team; key defaults to
  `reply:<ID>`, so answering twice sends one reply.
- `retract ID` — cancels a message not yet typed (this host's ledger).
- `status ID [--json]`, `outbox [--json]` — sender-side views.
- `pump [--retry-refused]` — retries deferred and ambiguous messages: asks
  the receiver first, resends the same ID only when the receipt says
  unknown, and never tries a second contact after ambiguity without a
  fence. `--retry-refused` only reports the successor key; a refused ID is
  never reopened.
- `seen ID` — this host's ledger state for an ID.
- `resolve ID (--fence | --mark-delivered)` — human-only settlement on the
  receiving host.
- `hook [--team TEAM]` — the harness hook for `hook-inbox`: claims items by
  rename, re-checks expiry and tombstones, prints what survives. Hook
  delivery never wakes an idle agent.

With the `agentctl-queue` deliverer, delivery starts on arrival: after an
item is enqueued, accept starts one bounded `agentctl drain
--deliver-while-working --ready-timeout 120` in a detached thread (never
more than one per queue, content-free entries in `team-log.jsonl`), so a
message does not wait for some other drain. Turn it off per declaration
with `"auto_drain": false` (and tune `"auto_drain_ready_timeout_seconds"`).

### Receiving host (run by routes)

- `serve --stdio --peer HOST` — the forced-command protocol endpoint. It
  prints a ready banner before reading a length-prefixed frame; verbs:
  `accept`, `receipt`, `fence`, `probe`, `ping`.
- `accept --protocol 1 (--envelope - | --envelope-base64 B)` — claims the ID
  in the ledger (`O_EXCL`, naming the hand-over target), applies policy,
  hands to the deliverer. A second arrival returns the recorded state; a
  different body under the same ID is a `conflict`. Refusals of the message
  are durable; refusals of the route (wrong host, unlisted relay) leave no
  ledger entry.
- `receipt ID [--team T]` — a pure read; never recovers.
- `fence ID --team T --not-after TIME` — bars an ID before it arrives, so a
  sender may move to another contact.

### ssh and links

- `ssh enroll --peer HOST [--from ADDR]` — creates a dedicated key under
  `~/.local/state/agent-teams/ssh/` (host state, never synced config) and prints the restricted `authorized_keys`
  line (`command="... serve --stdio --peer <this host>",restrict`).
  agent-teams never edits `authorized_keys`. Dedicated keys are optional.
- `link run ROUTE | up ROUTE | down ROUTE | status [ROUTE]` — managed
  persistent links (OpenSSH ControlMaster). Health lands in
  `links/<route>.json`; a link known down is skipped definitely. Run
  `link run` under a user service you install; agent-teams never runs
  `systemctl`.

## Routes

Drivers are built in (`local`, `ssh`, `command`); routes are data.

- `local` — built in; `serve` as a child process on this host.
- `ssh` — fields: `target`, `user`, `identity`, `known_hosts`,
  `connect_timeout_seconds` (default 10), `use_user_ssh_config`,
  `link.mode` (`none` default, `on-demand`, `persistent`; with
  `use_user_ssh_config` the owner's own master and `Control*` settings
  apply and are never overridden — agent-teams' managed socket is only for
  dedicated-key routes), `capabilities.max_envelope_bytes`. Without `use_user_ssh_config` the driver uses
  `-F none`, `BatchMode`, `IdentitiesOnly`, `StrictHostKeyChecking` with its
  own known-hosts file, and no forwarding. With `use_user_ssh_config: true`
  (allowed permanently) it uses the owner's configuration and agent; the
  receiver records the peer as asserted over owner ssh.
- `command` (host mode) — fields: `argv` template with `{remote...}`,
  `{vars.X}`, and `{host.address}` variables, `outcome.pre_exec_failure`
  entries (exit code plus a literal stderr prefix; only these are definite
  non-delivery), `interactive` (banner protocol when true).

Every attempt is `answered`, `not-delivered` (definite; for banner routes,
no banner means the envelope was never sent) or `ambiguous`. After ambiguity
the sender asks for a receipt; it never resends blind.

- `command` (relay mode) — the chat relay: fields `channel`, `sender_id`
  (the bot user this host posts as), `post_argv` and `list_argv` templates
  (`{channel}`, `{thread}`, `{message_id}`, `{text_file}`), 
  `max_envelope_bytes` (default 10000, one post per envelope, never split),
  `max_post_bytes` (default 3500; the whole post measured after escaping the
  visible text — an envelope whose fence alone exceeds it skips this route),
  `limits.per_hour` (default 6), `visible_to_owner: true`, and `mirror`
  (default false; see "Mirror copies" in the Chat relay section), and
  `post_timeout_seconds` (default 10; raise it when the helper waits out
  rate limits — a timed-out post kills the helper's whole process group).
  It posts the
  envelope as one chat post and never reposts: ambiguity is settled by the
  receipt in the post's thread, delivered back through the sender's bridge
  to `relay-in` (a channel listing has no authenticated poster, so the
  route's receipt verb never settles from one), or by a receipt over
  another route to the same contact. `list_argv` remains useful for
  operator diagnostics. Bodies are visible to
  every channel member, so `--private` messages never take a relay route,
  and the walk's other rules (order, pinning, failover to ssh/node-exec)
  apply unchanged: the relay is a route like any other, chosen last.

### Per-team routes and relay fallback

A host's `routes` list is only the default. A team may carry its own
ordered `routes` in teams.json, and one entry may carry its own, winning
over the team's: the entry's list wins, then the team's, then the host's,
and the relay still sorts last within whichever list applies. A relay
route named in a team's or entry's own list is the destination's
explicit opt-in. Every other relay route is kept only when BOTH teams
are **relay-enabled** for it — each one's declaration on the sending
host names a `relay_channels` entry for that route — and the sending
side is required even for an explicit list. A sender or destination
the sender cannot prove relay-enabled gets no relay route at all:
fail-closed, because relay routes expose the body. When every direct route is down,
a relay-enabled team's message therefore falls back to the relay and
waits in custody for its thread receipt instead of expiring unseen;
a team with no relay simply stays `deferred` for `pump`.

**Rolling this out takes an upgrade first.** Older builds ignore the
new teams.json keys and read only host route lists, with no relay
filter at all: adding a relay to a *host* list before every reader on
every host sharing that teams.json is upgraded routes every team over
the relay on those builds. Upgrade all builds on all hosts first; only
then add relay routes (per-entry lists preferred) or `mirror`.

**Rolling this out takes an upgrade first.** Older builds ignore the
new teams.json keys and read only host route lists, with no relay
filter at all: adding a relay to a *host* list before every reader on
every host sharing that teams.json is upgraded routes every team over
the relay on those builds. Upgrade all builds on all hosts first; only
then add relay routes (per-entry lists preferred) or `mirror`.

## Chat relay

The relay carries team messages through a shared chat channel, so each
direction between hosts has a path that depends on neither ssh nor a
node-exec runtime, and the owner can read the inter-team traffic there.
It is provider-neutral: any chat-subscription provider that reports each
post's authenticated sender ID, delivers other bots' posts in the channel
to the bridge, and can post through an outbound helper qualifies (Slack
and Discord included). Nothing provider-specific is in `agent-teams` or
in the bridge's relay class; channel, bot IDs, and helper argv are data.

**Layout.** One relay channel per fleet is the default: for example
`#relay-agent-utils`, with only that fleet's teams' bots as members. The
declaration's relay entry then carries `"fleet": "<fleet>"`, and a post
whose envelope names a different fleet, in either direction, is refused
there. A single channel shared by several fleets also works (leave
`fleet` unset); posts addressed to other fleets or hosts are filtered by
`relay-in` on the envelope's `to` without waking any agent — no model
sees them, so they cost no tokens. One bot account per team is the normal
case. Several sender IDs may map to the same team (a fleet's two bots on
Discord, say); every ID is mapped explicitly.

**The post.** One post per envelope: a short human-readable line (the
team tag and the body, so the owner can read the channel), then the full
envelope as compact JSON in a fenced `agent-teams-envelope` block. The
whole post is capped at the route's `max_post_bytes` (default 3500),
measured after escaping the visible text, so the outbound helper never
splits it in converted mode (its limit there is 4,000 bytes after
conversion; a split post's first chunk carries no fence): for a long body
the visible copy is truncated and says so (a long body shows only its
first ~500 chars, since the envelope already carries the full text, so a
post stays ~1x the body, not ~2x), while the envelope always carries the
full body. An envelope whose fenced block alone exceeds the
cap skips the route instead: a fallback stays deferred (its outbox
entry names the reason as `too large for relay route ...`) and a mirror
records `mirror-skipped`. `check` warns when `max_envelope_bytes` exceeds
`max_post_bytes`. Inside the fence the envelope JSON escapes the
characters Slack auto-links (`.`, `:`, `@`, `<`, `>`, `&`), so a body
naming a file like `cli.rs`, a URL or an email survives Slack intact;
a receiver also undoes Slack link markup from older posts and accepts
only when the body hash then verifies. A body-hash mismatch that
survives that undo, from a confirmed sender, is quarantined under its
own integrity alarm kind (dismissable, kept as evidence). A
signed envelope whose tag line was lost (the fence-only piece of a
split) is still delivered: the signature, not the tag, is the authority. The envelope is authoritative — a provider may rewrite
visible text, so the receiver re-derives the tag from the envelope and
checks the body hash.
Receipts go back in the post's thread (one line plus a fenced
`agent-teams-receipt` block); a receipt never draws a receipt.
Exactly-once comes from the envelope IDs and the receiving ledger, as on
every route: duplicate posts (a helper repost, a rescan, a replayed
stream) are absorbed by the ledger, never typed twice. Latency is that
of two bridges; when a provider's live stream does not carry other bots'
posts, its history rescan bounds it (Slack: 90 seconds by default, over a
24-hour window — a post older than the window is never seen, so keep
relayed messages' TTL inside it). The Slack provider can instead poll
just the relay channels: `relay_poll_seconds` (5-60) in the bridge's
Slack backend configuration lists only those channels' history and
threads with new replies on that cadence, with no settle wait and no
grace, through the same admission, ledger and listing budget. Without
it, a relay post after a quiet bridge restart can wait out the stream's
settle (up to about 10 minutes) plus up to an interval; with it, the
bound is one poll. Receipts ride the envelope post's thread, which the
poll lists whenever it gains replies.

**Mirror copies.** A relay route definition may carry `"mirror": true`.
Then, whenever a message is *delivered over a direct route*, the sender
also posts a copy through that relay route — the same signed envelope,
the same ID — so the owner can watch all inter-team traffic in the relay
channel, not only the messages the relay had to carry. Mirroring asks
for nothing in any host's route list: mirror routes are found in
`routes.d` directly. Only relay-enabled teams are mirrored, on BOTH
sides (the sending team's and the destination team's declarations on
the sending host must each name a `relay_channels` entry for that
route, and the sender must hold a signing key the receiver's bridge has
confirmed); `--private` messages are never mirrored; a message the
relay itself carried is not mirrored again; and replies are mirrored
exactly like sends. The attempt is recorded in the outbox before the
post runs (attempts with verb `mirror`, classified `mirror-posted`,
`mirror-skipped`, or `mirror-failed`) and as a `mirror` event in
`team-log.jsonl` (never the body); the delivery state is already
durable by then and the post command has a 10-second timeout. Mirror
posts count against their own allowance, `limits.mirror_per_hour`
(default 60), never the `limits.per_hour` a fallback needs, and land
in their own posted-ID set, so a failed mirror can never make a later
fallback ambiguous. A mirror failure or skip never changes the
message's delivery state, exit code, or retry behaviour, and `pump`
never retries a mirror.

On the receiving side, the mirror copy is a duplicate by construction:
the bridge feeds it to `relay-in`, the ledger has already claimed the ID
through the direct delivery, and the copy is absorbed quietly. It
delivers nothing a second time, raises no quarantine and no stranger
alarm, and its receipt in the thread keeps the recorded state with a
`<state> already` detail — for a delivered message, outcome `known` and
receipt `receipt: delivered` saying `delivered already; duplicate copy
not delivered again`. The copy must carry the real signed envelope,
exactly as the relay route's `accept` posts it; a plain-text rendering
of the message is not an envelope and would be quarantined as a
stranger.

**Admission, in three layers.** The provider admits a listed peer's post
in the relay channel (Slack: that peer's `bot_message`, there only). The
bridge's `relay_channels` class admits it only to one configured command
(`agent-teams relay-in --route R`, with `--peer-sender`, `--channel` and
`--thread` appended): never typed, never a prompt. `relay-in` then checks
the envelope against this host's declaration: the poster's mapped fleet
plus team (`FLEET@HOST`) must match the envelope's `from`, and any
claimed sender that is not confirmed is a stranger (below). A peer can
never inject prompts: its posts reach only the relay command, and the
coordinator sees only what its own deliverer renders from a verified
envelope.

**Security model.** The platform authenticates a bot's sender ID; it
cannot stop a bot in the channel from *claiming* any fleet or team in its
envelope text. So: a forged post from an unlisted or mismatched sender is
quarantined and alarmed, never delivered, and claims nothing in the
ledger — it cannot fence, block, or duplicate a legitimate message with
the same ID. A replayed post is absorbed by the ledger. A confirmed peer
can send only as its own fleet's team, only within the receiving team's
ordinary policy (sender rules, size, TTL, rate limits), and every body it
sends is visible to all channel members — relay routes are public to the
channel, which is why `--private` never takes one. Announcements
(below) are candidates only: their integrity hash proves a block was not
altered, never who sent it; identity comes from the poster ID matching
the announced sender ID, plus confirmation.

**Strangers are loud, never silent.** A post addressed to this team from
an unconfirmed sender — unlisted, mapped to another fleet/team, or with
an envelope that fails its tag, hash, or schema check — is a stranger
event. `relay-in` records it, keeps the post quarantined (not delivered),
and raises an alarm to the team's coordinator through its normal
deliverer, flagged for escalation to the owner through that team's normal
owner channel (the chat bridge). The alarm repeats on a timer
(`alarm_interval_seconds` in the declaration's relay entry, default
1800) whenever `relay-in`, `pump`, or `relay alarms` runs, until the
sender is confirmed (`relay confirm`) or banned (`relay ban`; `relay unban` reverses a ban, and confirming a sender also clears its ban). A banned
sender's posts are dropped without new alarms. `relay status` shows open
stranger alarms first. Quarantined posts addressed to us are delivered,
in arrival order and exactly once, as soon as their sender is confirmed —
nothing is silently lost. Posts for other fleets from confirmed senders
are still dropped silently (above); only strangers and malformed posts
are loud. A malformed post from a CONFIRMED sender (a split or truncated
relay post is the usual cause; a sender listed in a declaration's
`relay_channels[].peers` counts as confirmed) is not a stranger: it
raises a relay alarm of its own kind naming every quarantined post from
that sender, and is dismissed with `relay quarantine dismiss <id>`,
which also closes the alarm. An unparseable post never draws a reply
(its plain-text receipt would itself be unparseable), and a given post
is recorded at most once however often a rescan re-offers it.

**Discovery and trust.** Each team announces itself in the channel
(`relay announce`): fleet, host, the sender ID it posts as, and a
repository identity (SHA-256 of the normalized origin URL, so teams on
one repository find each other without naming it). `relay-in` records
announcements as candidates — never admitted. `relay teams [--repo ID]`
lists confirmed mappings and candidates, optionally for one repository.
`relay confirm SENDER` first asks the claimed team's own host over an
already-trusted route (ssh or node-exec): when that host's probe reports
the same sender-to-team mapping in its declaration, the mapping is
confirmed automatically. Otherwise the owner approves with one line
(`relay confirm SENDER --owner-approved`). Never admit an unconfirmed
claim. Onboarding is `relay setup`: it suggests the per-fleet channel
name (`#relay-<fleet>`), tells the owner exactly what to create and whom
to invite on Slack or Discord, and prints the bridge, declaration, and
route snippets to apply. Agents never create channels.

**Topology.** Name hubs under `topology.hubs`: spokes dial one persistent
link to the hub (n-1 links, not n²), pushing and pulling over it. Spoke to
spoke goes direct when an on-demand route exists. One relay at most (hub
relay is a later phase; `accepts_relay_from` already gates relayed
envelopes).

## Exactly-once rules

At most one enqueue and one paste per ID per team; at least once while some
route to a verified contact stays up before `not_after`; never silently in
between — an unprovable hand-over ends `possibly_submitted` or `lost`,
visible in receipts, and is never retried automatically. Ledger entries are
kept until 30 days past each entry's own `not_after`, so pruning can never
reopen an ID.

States include `claimed`, `accepted`, `typing`/`presented`, `delivered`,
`expired`, `superseded`, `retracted`, `fenced`, `possibly_submitted`,
`lost`, `refused`, and sender-side `deferred`, `ambiguous`,
`ambiguous-expired`, `awaiting-pull`, `in-custody`.

## Exit status

`send`, `pump` and `status`: 0 delivered; 75 not settled; 76 uncertain; 65
terminal and undelivered; 2 usage or configuration. `check`: 0 or 1.

## The team tag

Every rendered message starts with one fixed tag written by agent-teams —
`[from sender@host via agent-teams · to: team@host · id: tm-...]`, in the same
one-line sender-header grammar every `agentctl send` prompt opens with — then
a receiver-generated line (kind, topic, expiry, proof, route), then the body
with any line starting with `[` indented so a body cannot forge a header, then
a reply command. Agents never write the tag themselves.

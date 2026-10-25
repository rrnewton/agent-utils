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
  optional `topology` object naming star hubs. Stale data costs a warning,
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
    (default 7 days). There is no `--ttl none`.
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
  `~/.config/agent-teams/ssh/` and prints the restricted `authorized_keys`
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
  `link.mode` (`none` default, `on-demand`, `persistent`), `capabilities.
  max_envelope_bytes`. Without `use_user_ssh_config` the driver uses
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
`[sender host -> team host tm-...]` — then a receiver-generated line (kind,
topic, expiry, proof, route), then the body with any line starting with `[`
indented so a body cannot forge a header, then a reply command. Agents never
write the tag themselves.

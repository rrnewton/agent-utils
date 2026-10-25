---
name: agent-teams
description: Message another team's coordinator across hosts, exactly once, with agent-teams - and receive what other coordinators send your team. Use when a coordinator needs to hand work, a status, a question, or a hold/release to a coordinator on another host or in another harness, when replying to a team message typed into your pane, or when checking whether a team message arrived, was typed, expired, or went uncertain. Never use it from a worker session: workers ask their own coordinator.
---

# agent-teams

A team is a coordinator plus the workers it manages, on one host. `agent-teams`
carries a message from one team's coordinator to another team's designated
coordinator, across hosts, exactly once: the receiving host claims the message
ID in its team ledger before typing anything, so retries, second routes, and
racing drainers can never type it twice, and drainers that die before typing
leave the item queued, never lost.

The installed command is the authority for its own usage:

- `agent-teams quickstart`
- `agent-teams userguide`
- `agent-teams help`, `agent-teams send --help`-style help text per verb
- `agent-teams check` — declarations, contacts, routes, agentctl lane support
- `agent-teams show` — teams, hosts, routes, and freshness

## Who may send

- Only a team's **designated coordinator** sends and receives cross-team.
  Sender proof is your process ancestry: you must actually be running as the
  coordinator (or inside its managed tree). This is checked on both hosts, and
  the receiver delivers to its designated coordinator, never to whoever
  started the process that carried the message.
- Registered services may send operational alerts when the receiving team's
  declaration lists them (`--from-service NAME`).
- **Workers never send cross-team and never receive team messages.** If you
  are a worker and need another team, tell your own coordinator what you need
  and let them forward it. Lieutenants do the same: they ask their
  coordinator, who decides.

## When to message another team

- Handing off work that the other coordinator owns, with everything they need
  to start: goal, inputs, constraints, how to report back.
- A status the other coordinator is waiting on, a question only they can
  answer, or a `hold`/`resume` for work they run for you.
- Not for chatter, not for what your own workers should hear (use `agentctl`
  inside your team), and not as a substitute for the shared record: anything
  durable belongs in the project's issues and docs, with the message pointing
  at it.

## Send, reply, track

```bash
agent-teams send TEAM[@HOST] --file MSG.md --key HANDOFF-SLUG
agent-teams reply ID --file REPLY.md        # ID from the typed message
agent-teams status ID                       # outbox + receiver receipt
agent-teams pump                            # retry deferred/ambiguous
agent-teams retract ID                      # before it is typed
agent-teams seen ID                         # on the receiving host
```

- Give every logical message a stable `--key`. Retrying a send with the same
  key is safe and is how recovery works; a new key is a new message.
- Use `--topic SLUG` (with `--supersedes KEY`) when a newer message replaces
  an older untyped one, `--kind hold|resume` to pause or release the other
  team's work, and `--ttl` when the content goes stale: expired messages are
  set aside untyped, never delivered late.
- Exit status tells you where the message stands: `0` delivered (typed into
  the other coordinator's pane), `75` not settled yet (accepted, deferred,
  in-custody), `76` uncertain (`possibly_submitted`, `lost` — settle by hand
  with `agent-teams resolve` on the receiving host, never by resending under
  a new key), `65` terminal and undelivered (expired, superseded, retracted,
  refused).
- `accepted` means the receiving ledger has claimed the ID; it becomes
  `delivered` once typing is confirmed. A message that arrives while the
  coordinator's queue deliverer is an `agentctl` queue starts a bounded
  drain on arrival, so you normally do not wait for the other coordinator's
  next prompt cycle.

## What a received message looks like

Delivery types the message into your pane with the team tag as its first
line — never invent or alter this grammar when you quote or forward it:

```text
[FROM-TEAM FROM-HOST -> TO-TEAM TO-HOST tm-...]
[team message: kind info; expires ...; sender proved on FROM-HOST (harness-ancestry); via serve:FROM-HOST]
<body>
[reply: agent-teams reply tm-... --file FILE]
```

Answer with the `agent-teams reply` line it gives you. The reply carries its
own tag and threads back to the sender's coordinator.

## Routes and restraint

- Prefer the host routes (ssh, command transports, persistent links). They
  are the default, the audited path, and the ones with exactly-once delivery.
- A chat-DM route exists in the design but is **off** (owner decision,
  2026-10-25). If a future route ever carries team traffic over a chat
  channel the owner reads, use it sparingly: every such message lands in a
  person's conversation, so send one only when a coordinator-to-coordinator
  note genuinely needs that visibility, keep it short, and let the tag line
  do the labelling. Hub-and-spoke relay is likewise shelved: with few teams,
  direct routes suffice.
- Private route definitions live in git-ignored `*.private.json` files. Host
  state (ledger, outbox, declaration, seen-set) never lives under a synced
  config directory; `agent-teams check` warns if it does.

## Failure reading

- `deferred` + exit 75: no route has settled yet. Check `agent-teams check`,
  then `agent-teams pump`; do not start a second copy under a new key.
- `possibly_submitted` (76): a drainer may have typed and died before
  proving it. It is never replayed automatically. Look at the receiving
  pane/queue, then settle with `resolve --mark-delivered` or `--fence` on
  the receiving host.
- `refused` (65): the receiver's policy said no (sender, kind, size, rate
  limit, or `may_send_to` on your side). Fix the declaration or your
  message; resending unchanged only refuses again.
- Nothing arrived anywhere: `agent-teams status ID` on the sender, `seen ID`
  on the receiver, and `agent-teams check` on both ends separate "never
  sent" from "sent, not yet typed" from "typed, receipt not yet folded".

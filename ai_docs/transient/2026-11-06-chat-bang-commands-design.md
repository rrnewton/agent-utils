# #169 Owner talks to subagents from Slack — Phase A design (mu-bang)

Status: Phase A design only. No code changed, no slot used, no live Slack touched.
Code base read: `agent-utils/rs/agentctl` at harness checkout (binary cache source
`17b67119bc39`). All citations are file:line in that checkout. Times ET.

## 0. Summary of picks (confirming the brief, with two refinements)

- Prefix `!`, permanent-owner only, handled deterministically in the bridge, never
  typed into any pane. Refinement 1: only `allowed_senders` (permanent owners)
  may use `!`; temporary grant holders (`chat_runtime.rs` `SenderGrant`) may not —
  the brief says "the bridge's allowed owner sender", and grants are explicitly
  lesser authority in the agentctl-chat skill.
- Refinement 2: when `outbound_enabled` is false the bridge cannot answer, so `!`
  messages fall through to normal admission instead of being silently eaten.
  Eating owner text with no possible reply is worse than delivering it.
- v1 scope stays local-registry only. Cross-host lieutenants (agent-teams) are v2.

## 1. Open questions resolved with evidence

### 1.1 Where do "managed-by" and "pinned" live today?

**Managed-by exists and is sufficient. No new edge is needed.**

- Each agent record (`agent.json`, flat `extra` map or nested-row extensions) can
  carry `created_by` and `managed_by` edges:
  `subagents/control.rs:23-24` (`CREATED_BY`/`MANAGED_BY`), `Edge{agent, registry,
  token, proof}` at `control.rs:37-50`. Edges name a generation by token and
  survive rename/revive via `former_tokens` (`control.rs:9-12`, `AgentRecord::edge`
  / `set_edge` at `control.rs:545-575`).
- Written by `start` (caller proved by harness-process ancestry) as both
  `created_by` and `managed_by`, and by `adopt` as `managed_by`
  (`cli.rs:117` help text; `control.rs` `creation_edges`, `caller_edge`;
  profile opt-out `record_edges` in `profiles.rs:40`).
- Changed explicitly with `agentctl manage NAME [--by M | --clear]`
  (`cli.rs:117`); cycles are refused.
- The transitive tree already exists: `ManagedAgents::tree()` (`control.rs:1448ff`)
  puts the designated coordinator (`REGISTRY/coordinator.json`, schema
  `agentctl-coordinator/v1`, `control.rs:19-21`) at the root, hangs each agent
  under its nearest in-registry `managed_by`, and hangs unmanaged / manager-absent /
  manager-stopped agents under the coordinator. `render_tree` prints it, and
  `agentctl tree --json` emits `name, token, lifecycle, harness, coordinator,
  control, created_by, managed_by, harness_pid, children`. Watch-agents already
  routes notices up the same chain (`controller_of`, `control.rs:677ff`).
- Verified live on this host: `agentctl tree --json` returns coordinator `coordb`
  with `managed` children carrying exactly those fields.

So `!who` is a rendering of `tree()` plus per-record detail, not a new data model.

**"Pinned" does not exist as an agent property. Two unrelated "pin" meanings do:**

1. Pinned harness process: `harness_identity` / `custom_process_identity`
   (`CustomProcessIdentity`) on the record (`subagents.rs:1801-1809`) — the process
   whose descendants act for the agent (`control.rs:634ff`). Every running agent
   has one; it does not distinguish lieutenants from workers.
2. Pinned coordinator designation in agent-teams declarations
   (`control.rs:251ff`) — cross-host routing, not a lane property.
3. `agentctl hold` is the landing queue (`REGISTRY/landing-queue.json`), not a pin.

The closest existing "persistent" signal is `REGISTRY/restore.json` (`agents[]`
desired state, revived at boot by `restore.rs` `spec_write_agents` #742/#810):
agents listed there are by definition the ones expected to survive a reboot.

**Proposal (smallest addition that matches the owner's words):** add one optional
record key `pinned: bool` (absent = false), preserved across rewrites by the
existing forward-compat kept-keys machinery (`forward_compat::KeptKeys`, #809):

- Set at `start --pinned`, or from a launch profile key `"pinned": true`
  (profiles already carry optional `role`/`protocol` keys, `profiles.rs:1049-1050`,
  so one more optional key fits the shape).
- Toggled later by `agentctl manage NAME --pin | --unpin` (manage already owns
  control metadata; no new top-level command).
- v1 display default when the key is absent: an agent renders as Pinned if it is
  the designated coordinator, is listed in `restore.json` `agents[]`, or manages
  at least one other agent (an interior lieutenant node); otherwise Ephemeral.
  Once the key exists it wins. This makes `!who` correct on today's registry with
  zero migration, and gives owners an explicit override going forward.
- `!who` never writes this (or anything) to the registry. Pinning stays an
  agentctl operation, not a chat side effect.

### 1.2 Where do harness / model / state / host for `!who` come from?

- `state`: record `lifecycle` (`running|starting|stopped|...`), as in `tree()`.
  Detail view adds the live classification the status command computes
  (working/idle/blocked) only where already available; v1 keeps lifecycle to stay
  deterministic and cheap — no Herdr screen reads for a `!who`.
- `harness`: record `harness`. `model`: record `model`, falling back to the launch
  profile's model — exactly the precedence `BridgeState::delivery_stamp_now`
  already implements for #897 stamps (`chat_runtime.rs:6369ff`: record fields,
  then profile, else `unknown`). Reuse that helper's field order; never guess.
- `host`: no host field exists on a record, and v1 is one registry on one host, so
  host is the bridge host's hostname, stated once in the `!who` header
  (e.g. `host devbig014`), not repeated per row. v2 gets real per-agent hosts from
  agent-teams declarations.

### 1.3 How is a thread binding stored durably?

New file in the bridge state directory (beside `bridge.json`, `requests/`,
`replies/`, `target.json`): `thread-bindings.json`, schema
`agentctl-chat-bindings/v1`, written with the same `write_document` + `.state.lock`
discipline as every other bridge document, unknown top-level keys kept (#809).

```json
{ "schema": "agentctl-chat-bindings/v1", "bindings": [
  { "channel_id": "C…", "thread_id": "C…/threads/171…",
    "agent_name": "mu-bang", "agent_token": "1793…",
    "root_message_id": "171…", "opened_at_millis": 0,
    "state": "active",               // active | ended
    "ended_reason": null,            // owner-end | agent-stopped | superseded
    "parent_notice_id": "chat-bind-<sha256(thread_id)[..16]>",
    "parent_notified": true }
] }
```

- Keyed by `(channel_id, thread_id)`; the binding thread for `!@NAME` is the
  thread rooted at the owner's `!@NAME` message itself
  (`<channel>/threads/<that message's ts>`), so no provider call is needed to
  invent a thread — the bridge just starts replying in it.
- `agent_token` pins the generation (same `answers_to` / `former_tokens`
  resolution as edges), so a revive keeps the binding and a name-reuse by a new
  generation ends it instead of misrouting owner text to a stranger.
- One active binding per thread; an agent may hold several threads (each gets its
  own parent notice, per the brief's "per new bound thread"). A second `!@NAME`
  for an agent with an active thread still opens a new thread — the brief's
  `!@NAME <msg>` "opens a NEW Slack thread" is explicit.
- The file is bridge state, not registry state: bindings die with the bridge,
  never outlive it into another bridge serving the same channel, and are covered
  by the existing state-directory privacy (owner-only, single-linked checks).

### 1.4 Reply capture per harness; #896 receipt-evidence rules

Today the bridge is single-target: `BridgeConfiguration.agent_name`
(`chat_runtime.rs:699-700`) is the one coordinator, and `BridgeState` holds one
transcript follower, one reply source, one reply mode (`chat_runtime.rs:5951ff`).
Reply text comes from, in order: the target's harness transcript
(`chat_transcript.rs`, #710), else its screen; headless and unknown-harness
targets get command-only prompts (`reply_mode()`, `chat_runtime.rs:10222ff`, #896:
receipt ✅ only with evidence the pane printed the prompt, or a headless model
started it; `transcriptless_target` targets still have screen blocks captured).

Design: generalize, don't fork. Each admitted request gains an optional
`target_agent` (+ token) on its `RequestRecord` (absent = the bridge's
`agent_name`, so every existing record and code path is unchanged). The owner
loop then works per target:

- Delivery: the existing `CoordinatorDelivery` trait already takes `agent_name`
  on every call (`submit`, `drain`, `screen`, `withdraw`, …,
  `chat_runtime.rs:5578ff`), and `ManagedAgents::send_identified` / `drain`
  already deliver to any registered agent through the same durable queue. Bound
  requests are queued and typed exactly like coordinator requests, with the
  identical prompt anatomy (header, `<chat_message>`, marker/footer) — the
  subagent cannot tell a bound chat request from a coordinator one, which is the
  brief's "normal reply protocol … irrespective of harness/agent type".
- Capture: screen scanning is already alias/nonce-keyed bridge-wide
  (`capture_snapshot_with_gate`), so blocks from different panes never collide;
  the loop reads each active target's pane. Transcript following becomes a small
  map `agent_name -> follower` reusing `chat_transcript::CoordinatorTranscript`
  and its `retarget` logic (#852); v1 follows the coordinator plus agents with an
  active binding, and drops a follower when its last binding ends.
- Reply mode is computed per target with the same `reply_mode()` rules
  (headless → command-only; unknown harness → command-only; transcript found →
  blocks; transcript looked-for-and-missing → command-only). #896 is preserved
  per request: the ✅ receipt for a bound request requires the same evidence
  (prompt verified in that agent's composer, or its headless queue showing the
  model started it) — never mere queue acceptance.
- `chat reply N` path unchanged: `chat_service::reply` stores by request key and
  wakes the service; the service sends into `record.message.thread_id`, which for
  bound requests is the bound thread. One required fix: bridge discovery for
  `chat reply`/`chat sent` currently keeps bridges "whose target agent is you"
  (`cli.rs` chat-reply help, `chat_lookup.rs`); it must also match a bridge
  holding an open request whose `target_agent` is the caller, else a subagent's
  short `agentctl chat reply NNN` cannot find its bridge. The explicit
  `--bridge-state … --request KEY` form already works and is what bound prompts
  print when caller lookup would be ambiguous (same fallback as repoint-era
  prompts).

### 1.5 Interaction with the existing per-request protocol and `chat reply N`

- Nothing about the coordinator path changes: unbound messages admit, prompt,
  capture and reply exactly as today. `!` handling sits beside the existing
  relay-channel and ignored-prefix branches in `admit_batch_scoped`
  (`chat_runtime.rs:8031ff`), after channel-authority and permanent-owner checks,
  before `RequestRecord::from_saved_message`.
- Each bound-thread message is a full request: own reply alias from the one
  bridge-wide alias space, own `<CHAT_REPLY_NNN>` markers, own `chat reply N`,
  `chat sent N`, `chat thread N`. No new reply protocol is invented.
- Deterministic command answers are not requests and get no alias: they are
  posted with the outbound helper under a deterministic operation UUID derived
  from the inbound message ID (same idempotency discipline as reply sends and
  rejection reactions), journaled in the admission outcome so a provider replay
  of the same batch cannot post twice. Admission stays provider-agnostic: it
  records command candidates under the state lock; the service posts them after
  commit confirmation, mirroring `rejections.rs` candidates.
- Existing `chat publish` (`RootMessageTransport::publish_in_thread`,
  `chat_runtime.rs:3845ff`) is the precedent for bridge-authored threaded posts
  that create no reply route; command answers follow it but reuse the reply
  transport's operation-ID store so retries reconcile instead of duplicating.

## 2. Command grammar (deterministic, owner-only)

Parsed from the message text after trimming trailing whitespace; the text must
start with `!` at the first character (Slack does not trim leading spaces into
existence, and a quoted/forwarded `!` mid-text must never trigger). Agent names
use `agent_queue::registry::is_agent_name`, the same grammar as everywhere else.

| Input | Behavior |
|---|---|
| `!help` | Post command list (below) in a thread under the `!help` message. |
| `!who` | Post the managed-by tree (see §3) in a thread under the message. |
| `!who NAME` | Post NAME's detail card (see §3); unknown → §5 behavior. |
| `!@NAME` | Open bound thread; first agent message is a bridge-authored "say hello" prompt? No — see note. |
| `!@NAME <msg>` | Open bound thread; deliver `<msg>` to NAME as its first request. |
| `!end` (in bound thread) | End that binding; post confirmation in the thread. |
| `!end` (elsewhere) | Post "this thread is not bound; use `!@NAME`…" help line. |
| `!anything-else`, bare `!` | Post the `!help` text (threaded), deliver nothing. |

Note on bare `!@NAME` with no message: opening a thread with nothing to say
would type an empty prompt into the pane. Instead the bridge opens the binding,
posts "Thread bound to NAME — write here to talk to it" in the new thread, and
delivers nothing until the owner's first real message in the thread. `!@NAME
<msg>` additionally delivers `<msg>` at once. This keeps "every pane delivery is
a real owner message" true.

Case: commands are lowercase-exact (`!who`, not `!WHO`); a differently-cased
`!Who` falls into the unknown-command branch and gets `!help`, which teaches the
spelling. Names are case-exact as registered.

Where commands may appear: any configured non-relay channel the bridge serves.
In relay channels the existing relay rules win and `!` text is never a command
(relay channels are "not an owner channel", `chat_runtime.rs` admit comments).
Grant-holder senders: `!` text from a non-owner is not special at all — it is
admitted as an ordinary request to the coordinator, exactly as today, so a
grantee cannot probe the agent tree and the coordinator still sees their text.

## 3. `!who` rendering

Source: `ManagedAgents::tree()` + per-record detail, read once under no lock
(records are replaced atomically; the tree code already tolerates unreadable
records by skipping them). Output is plain text sized for Slack: no tables (the
screen-table refusal in the bridge exists because tables mangle; chat posts use
the Slack format the bridge already applies).

```
Subagents of coordb (host devbig014) — 7 agents
Pinned (persistent lanes):
coordb — coordinator — claude/opus — running
└── liaison — claude — running — manages 2
Ephemeral:
├── mu-bang — muse/watermelon — running — managed by coordb
└── hands — claude — idle — managed by coordb
Talk to one: !@name <message> · Details: !who name · Help: !help
```

Rules: tree indentation preserved (managed-by edges only, never Herdr tabs);
the Pinned/Ephemeral split is a section split, not a re-parenting — children stay
under their manager inside whichever section the child itself falls in, with a
`managed by X` suffix when the manager landed in the other section (avoids
duplicating subtrees). Each row: name, harness/model (`unknown` when no source
gives it), lifecycle. Depth cap and agent cap: render at most 50 agents, then
"… and N more — `!who NAME` for one agent"; the tree walk inherits the existing
64-depth cycle cut. `!who NAME` adds: token-short, profile, cwd, pane, created,
managed-by/created-by edges with proof, pinned source (explicit/restore/derived),
active bound thread if any, and the exact `agentctl read NAME` /
`agentctl status NAME` commands.

## 4. Delivery and reply flow for a bound thread

1. Owner posts `!@mu-bang check the gate` (top level). Admission recognizes the
   command, journals a binding `(channel, <channel>/threads/<msg ts>) → mu-bang`
   with `parent_notified=false`, and creates the first request with
   `target_agent=mu-bang` and the binding thread as its `thread_id`
   (`thread_reply=true` synthesized; the provider root is the `!@` message).
2. After commit, the service: (a) posts the deterministic ack in the new thread
   (`Bound to mu-bang. Replies here go to it; !end ends this. Parent coordb has
   been told once.`), (b) sends the parent notice (§5), setting
   `parent_notified=true` durably before the send is considered done, (c) the
   owner loop delivers the request to mu-bang's pane/queue like any request.
3. Owner writes more in the thread: admission sees an active binding for
   `(channel, thread_id)` + permanent-owner sender → request with
   `target_agent=mu-bang`. Non-owner senders in a bound thread are never routed
   to the subagent (ordinary rejection path), since Slack threads are visible to
   the channel.
4. Mu-bang replies between its markers (or `chat reply N`): captured from its
   transcript/screen, stored as replies of that request, sent into the bound
   thread. Rate breaker, held-reply, and send-retry behavior are the existing
   per-thread machinery, unchanged.
5. `!end` in the thread (or auto-unbind, §6) marks the binding `ended`; later
   messages in that thread admit as ordinary coordinator requests (today's
   thread behavior), with one deterministic line posted at unbind time saying so.

## 5. Parent notice (exactly one per new bound thread)

Sent once, through the durable agent queue (not chat state): `agentctl send`
semantics via `ManagedAgents::send_identified` with deterministic message ID
`chat-bind-<sha256(thread_id)[..16]>`, so any retry/replay dedups in the queue.
Recipient: the agent's manager from its `managed_by` edge, else the designated
coordinator; no notice when the parent would be the bound agent itself or when
the owner bound the coordinator (talking to the coordinator in a side thread is
already the coordinator's own business — flagged as an open pick for coordb if
they disagree). Body:

```
[chat-bridge] The owner is talking directly to mu-bang in a bound Slack thread.
You are not in that loop. Catch up any time:
  agentctl read mu-bang
  agentctl chat thread --bridge-state <DIR> --thread <THREAD_ID>
This notice is sent once per bound thread.
```

Stored `parent_notified` is set only after the queue accepts the message; a
failed send leaves it false and the next pass retries (still exactly one queued
copy, thanks to the deterministic ID). If the parent agent is stopped, the queue
holds the notice as it holds any prompt; auto-unbind rules below still apply.

Unknown name (`!@nope`, `!who nope`): one threaded deterministic reply with
near-matches (exact-prefix, then substring, then edit-distance ≤2 over registered
names, max 5) plus the full `!who` list (capped as in §3). No binding, no notice,
no delivery.

## 6. Failure modes

- Agent stops while bound: owner loop notices the record gone/stopped (same
  liveness the watcher uses), marks binding `ended/agent-stopped`, posts one
  line in the thread. Messages already queued to it stay in its agentctl queue
  (normal semantics); new thread messages fall back to the coordinator.
- Agent revived as a new generation / name reused: token mismatch → same as
  stopped. Revive of the same generation (`former_tokens` answers) → binding
  follows silently.
- Agent renamed: binding follows by token; `!who` shows the new name; the thread
  gets one line naming the rename (posted on next owner message, not spammed at
  rename time if the thread is idle — recorded as pending note on the binding).
- Bridge restart: bindings reload from `thread-bindings.json`; followers rebuilt;
  `parent_notified` and deterministic command operation IDs prevent duplicates;
  unsent bound replies follow the existing held-reply rules (#781).
- Repoint (`chat repoint`): moves the bridge's own coordinator target only;
  bindings and their targets are untouched. Parent-notice recipient is resolved
  live at send time, so a coordinator handover between open and notice is safe.
- Manager cycle / corrupt record: tree code already cuts cycles; a corrupt
  target record makes `!@NAME` answer "registered but unreadable" deterministically
  and create nothing.
- Outbound helper failure on a command answer: operation stays pending with its
  deterministic UUID and retries like a reply send; the command is never
  re-admitted as a coordinator request on replay (the journaled outcome is the
  dedup).
- Prompt-gate subtlety: capture gating ("blocks read before their prompt reached
  the agent are withheld") is per request key, so a subagent pane showing an old
  `CHAT_REPLY` block from a coordinator-era request cannot leak into a bound
  request. Bound prompts typed into a working agent queue behind its current
  turn — normal drain semantics, owner-first ordering preserved per agent.
- Two bridges, one pane: existing reply-alias ranges keep marker routing apart;
  bound prompts print the explicit `--bridge-state` reply form when caller
  lookup is ambiguous (see §1.4 fix).

## 7. Tests (fake backend only; never live Slack)

Harness: `chat-subscription-fake` `FakeBackend` for subscription-level tests,
`BridgeState` unit tests in `chat_runtime.rs`, and owner-loop tests in
`chat_service.rs` with the existing fake Herdr/transport doubles. Durable tests
land with each step (smallest focused test per behavior, per repo convention):

- Step 1 (!help/!who): parsing matrix (owner vs grantee vs stranger; relay
  channel; cased/unknown/bare `!`); tree rendering (nested lieutenants,
  cross-section manager suffix, caps, cycle); pinned classification
  (explicit > restore > derived); admission journals no request for commands.
- Step 2 (!@NAME): binding created + first request targets the agent; ack and
  exactly-one parent notice (restart between open and notice still one);
  thread follow-ups route to the agent, non-owner thread posts do not;
  unknown-name reply with near-matches; per-harness reply modes (transcript,
  screen-only, headless command-only) and #896 receipt evidence per target;
  `chat reply` caller-lookup fix.
- Step 3 (!end/auto-unbind): owner `!end`; agent stop/rename/name-reuse;
  post-end fallback to coordinator; binding file corruption refuses loudly
  rather than misrouting.

## 8. Landing plan (each step gated, SHAs reported)

0. This design. (Coordb/owner may veto the two refinements in §0 and the
   bare-`!@NAME` no-delivery pick in §2.)
1. `!help` + `!who`/`!who NAME` + `pinned` record key/profile key + read-only
   tree rendering. No bindings yet.
2. `!@NAME` threads: bindings file, `target_agent` on requests, per-target
   delivery/capture/reply-mode, parent notice, `chat reply` lookup fix.
3. `!end` + auto-unbind + rename/revive handling + userguide/skill updates
   (agentctl-chat skill gains one paragraph: bound threads behave identically).

Per step: `cargo fmt --check`, `python3 scripts/check_client_names.py` (the bare
client-names check), focused `cargo test -p agentctl chat` tests, then
`make validate` run bare (never piped) in the slot before landing via the
harness `bin/land-main`. Slot work waits for coordb to create the slot (wrkslots
recovery fix) and brief Phase B.

## 9. v2 notes (explicitly out of scope)

Cross-host lieutenants via agent-teams declarations (real per-agent host,
remote binding delivery); binding a thread the owner starts by hand (thread-first
instead of `!@`-first); `!who --all-hosts`.

## Addendum: coordb rulings (2026-11-05, ET)

Approved as written, including: permanent owners only; fall-through when outbound is
disabled; `pinned` key + profile key + `manage --pin/--unpin` with the derived default;
bare `!@NAME` binds without delivering; no parent notice when the bound agent is the
coordinator or would notify itself. Host note: this host's Slack bridge targets the
liaison (`agent_name=liaison`) while the designated coordinator is coordb. The `!who`
tree root is the designated coordinator, the liaison renders as a pinned node, and a
parent notice for a coordb-managed agent goes to coordb, never to the liaison merely
because it is the bridge target. Issue: #917 chat-bang-commands.

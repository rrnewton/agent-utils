# Related work for `agent-teams` (draft)

Draft, 2026-10-25, for `#496 agentctl-team-messaging`. `AGENTS.md` requires a `RELATED_WORK.md`
for every top-level tool; this is the draft for the future `agent-teams` binary, written with
revision 3 of the design
([`../2026-10-24-agentctl-team-messaging.md`](../2026-10-24-agentctl-team-messaging.md)). It moves
to the tool's own directory when the tool is created. Only public, open-source systems and public
standards are compared.

The question each entry answers: how does the system carry a message from an agent (or its host)
on one machine to an agent on another, how many standing connections does that take, can a
message pass through an intermediary, and what stops a retry from delivering twice?

**How each claim was checked.** "Read" means the source or documentation was read at the commit
given, from a shallow clone made on 2026-10-25 (Gas Town from a clone made 2026-10-24). "Recalled"
means it is stated from general knowledge and was not re-read for this draft; the IETF texts
could not be fetched while it was written, so the RFC sections below are marked recalled and
should be checked against the RFCs before this file moves to the tool. Licenses were not
re-checked.

## Summary

| System | Cross-host shape | Standing connections | Intermediary hops | Duplicate control |
|---|---|---|---|---|
| Gas Town | one town host; remote workers reach it through an mTLS proxy; towns share state through a Dolt hub | star (workers to town) | none for messages | two-phase delivery labels; acks idempotent |
| MCP Agent Mail | one HTTP server every agent calls | star (agents to server) | none | none for sends found (server assigns IDs) |
| Ruflo (Claude Flow) federation | peer-to-peer WebSocket sessions between installations | pairwise, on join | none ("no DERP-equivalent relay") | per-envelope ID and nonce; no receiver seen-set found |
| AutoGen distributed runtime | one host servicer; workers open a gRPC stream to it | star (workers to host) | the host relays every message, in memory | none; pending requests are cancelled on disconnect |
| A2A | a client calls a remote agent's HTTP endpoint | none (per call) | none in the protocol | `messageId` "MAY" be used to detect duplicates |
| NATS | full-mesh clusters, gateways between clusters, leaf nodes in acyclic trees | mesh inside a cluster; tree for leaves | one hop inside a cluster | JetStream `Nats-Msg-Id` inside a duplicate window |
| MQTT bridges (Mosquitto) | a broker connects to another broker as a client | one per bridge | any graph the operator configures | QoS 2 per hop; loops left to the operator |
| Matrix federation | every server sends directly to every other server in a room | none (per transaction) | none | `txnId` per server pair; event IDs |
| Tailscale DERP | encrypted packets relayed when no direct path exists | each node to its home relay | one relay, carrying ciphertext | not a message layer |
| OpenSSH `ProxyJump`, `ControlMaster` | an ssh session tunnelled through a jump host; a shared master connection | one master per host, optional | one or more jump hosts, carrying ciphertext | not a message layer |
| SMTP, UUCP and netnews | store-and-forward between relays | none (per transfer) | many; recorded in trace headers | `Message-ID` history (netnews); loops counted (mail) |

## Agent systems

### Gas Town

[steveyegge/gastown](https://github.com/steveyegge/gastown), read at `c0b37bd` (2026-10-24).

- **One town per host, one database per town.** "One Dolt SQL server per town serves all
  databases via MySQL protocol on port 3307"; "If Dolt runs on a different machine (e.g., over
  Tailscale), set `GT_DOLT_HOST`" (`docs/design/dolt-storage.md:13-51`). Mail is stored as beads
  in that database and routed by address (`internal/mail/router.go`: town-level addresses go to
  the town's `.beads`, rig-level ones to the rig's); after writing, the router waits up to
  `DefaultIdleNotifyTimeout` (3 s) for the recipient's session to go idle and otherwise falls back
  to a queued nudge (`router.go:33-35`). Every agent is a tmux session on the town host, so mail
  and nudges never cross hosts; the database may.
- **Two-phase delivery.** A sent message carries `delivery:pending`; the recipient's ack writes
  `delivery-acked-by:`, `delivery-acked-at:` and finally `delivery:acked`, reusing an existing
  timestamp so that "retries produce the exact same label set instead of appending duplicate
  timestamps" (`internal/mail/delivery.go:12-60`). That makes the ack idempotent; nothing found
  makes the send idempotent across a lost answer.
- **Remote workers: a star with the town as hub.** Sandboxed workers in remote containers call
  `gt` and `bd` through `gt-proxy-client`, which forwards argv over mTLS to `gt-proxy-server` on
  the town host; the server accepts only certificates signed by its own CA with
  `CN=gt-<rig>-<name>`, "injects `--identity <rig>/<name>` (extracted from cert)", and "maintains
  an explicit allowlist of permitted subcommands — no arbitrary shell execution"
  (`docs/design/sandboxed-polecat-execution.md:172-200`, `docs/proxy-server.md`). The container
  "never contacts GitHub"; git also goes through the proxy.
- **Between towns: shared state through a hub, not messages.** Federation is "Partially
  implemented": Dolt remotes pointing at DoltHub exist; "Remote registration (gt remote add)",
  "Cross-workspace queries" and "Delegation primitives" are unchecked
  (`docs/design/federation.md:3, 95-106`). The Wasteland links towns through a shared DoltHub
  database that each rig forks, writes and syncs (`docs/WASTELAND.md:1-20`). No town-to-town
  message delivery was found.

**Implication.** Gas Town's cross-machine answer is a hub: a database every agent reaches, or a
proxy on the town host that remote workers dial out to. Its proxy is the closest public analogue
of the restricted forced-command key: the credential names the caller, the server injects that
identity, and only allowlisted commands run. Its delivery tracking is on the ack side; the send
side, where I5's duplicates came from, is not idempotent.

### MCP Agent Mail

[Dicklesworthstone/mcp_agent_mail](https://github.com/Dicklesworthstone/mcp_agent_mail), the
Python edition, read at `83ed8d7` (2026-10-23). Its README says it is superseded by a Rust edition
(`mcp_agent_mail_rust`), which was not read.

- **One server, every agent a client.** "A mail-like coordination layer for coding agents, exposed
  as an HTTP-only FastMCP server" (README:9), backed by a Git repository and SQLite. Agents on
  other machines reach it over HTTP; for production it belongs "behind a reverse proxy (e.g.,
  NGINX) with TLS termination", with a static bearer token or JWT and a reader/writer role split
  (README:2185-2195).
- **Contacts between projects need consent.** Agents in different projects link with
  `request_contact` / `respond_contact` before messaging (README:176-177, 398).
- **No send idempotency found.** `send_message(project_key, sender_name, to, subject, body_md,
  ...)` takes no client-chosen ID; `acknowledge_message` takes the server's integer `message_id`
  (README:2289-2297). A retry after a lost answer would, by that signature, be a second message
  (inferred from the signature; not tested).

**Implication.** The simplest cross-host topology, a star around one always-on server, works
because the server is the only store. It makes the hub a single point of failure and the place
every body lives, which is the trade-off the hub relay in the design has to state.

### Ruflo (Claude Flow) federation

[ruvnet/claude-flow](https://github.com/ruvnet/claude-flow) (now published as Ruflo), read at
`c365d37` (2026-10-25).

- **Peer to peer, no directory.** Installations "discover each other, exchange signed manifests,
  and send messages between them"; "Ed25519 identity ... No central directory"; transport is WSS
  (`docs/federation/README.md:7-16`). A peer joins another's endpoint
  (`federation join wss://team-b.example.com:8443`, README:385-392).
- **Trust ladder and budgets per peer**, with a circuit breaker that suspends and evicts failing
  peers (`docs/federation/README.md:12-13`).
- **No relay.** The optional WireGuard mesh lists "No NAT traversal — peers must be direct-UDP
  reachable" and "No DERP-equivalent relay — if direct UDP fails, fall back to Tailscale instead"
  (`docs/federation/phase7-mesh-bringup.md:192-193`).
- **Envelopes are per call.** The routing service creates a new `envelopeId` and nonce for each
  send and HMAC-signs them with the session token
  (`v3/@claude-flow/plugin-agent-federation/src/domain/services/routing-service.ts:87-112`). A
  receiver-side seen-set of envelope IDs or nonces was not found in the plugin's sources (a
  search for `nonce`, `replay` and `dedup`); a retry creates a new envelope.

**Implication.** A pairwise model with no relay pushes reachability to the network layer
(Tailscale) and gives every pair its own session, the n² shape the owner wants to avoid for
persistent links. Its per-peer trust levels resemble the receiving declaration's `accepts_from`.

### AutoGen distributed runtime

[microsoft/autogen](https://github.com/microsoft/autogen), read at `004749e` (2026-10-16).

- **A host servicer and workers.** "The host service maintains connections to all active worker
  runtimes, facilitates message delivery, and keeps sessions for all direct messages (i.e.,
  RPCs). A worker runtime ... connects to the host service. It also advertises the agents which
  they support" (`python/docs/src/user-guide/core-user-guide/framework/distributed-agent-runtime.ipynb`;
  marked experimental). Workers open a bidirectional gRPC stream (`rpc OpenChannel (stream Message)
  returns (stream Message)`, `protos/agent_worker.proto:128`).
- **The hub relays, live only.** The servicer looks up the target worker's send queue, forwards
  the request, and keeps a future per `request_id`; when a worker disconnects, "Cancel pending
  requests sent to this client" (`python/packages/autogen-ext/src/autogen_ext/runtimes/grpc/_worker_runtime_host_servicer.py:109-140,
  232-264`). Queues are in memory (`asyncio.Queue`).

**Implication.** The canonical star for agents: workers dial out, the hub routes by advertised
agent type, and spokes need not be reachable. It has no custody: a message in flight when a
worker drops is cancelled, not stored. The design's hub keeps the star and adds a durable spool,
so that a spoke that is down receives the message when it returns.

### A2A (Agent2Agent protocol)

[a2aproject/A2A](https://github.com/a2aproject/A2A), specification read at `98279d6` (2026-10-20).

- **Direct calls to a discovered endpoint.** Agents publish an Agent Card at
  `https://{server_domain}/.well-known/agent-card.json` (`docs/specification.md:1988, 3355-3379`)
  and are called over JSON-RPC, gRPC or REST. There is no relay or routing layer in the protocol.
- **Idempotency is optional.** "Send Message operations MAY be idempotent. Agents may utilize the
  messageId to detect duplicate messages" (`docs/specification.md:495`). Push notifications are
  at-least-once: "Clients SHOULD process notifications idempotently, as duplicate deliveries may
  occur" (`:878`, `:3152`).

**Implication.** A2A assumes reachable HTTPS endpoints and leaves duplicate suppression to the
receiver as an option. The design makes the receiver's seen-set mandatory, because its routes
include ones (ssh, a node-exec command, a chat post) with no idempotency of their own.

### Not compared

OpenAI's Agents SDK handoffs pass control between agents inside one program run (recalled from
its public documentation, not re-read); they carry no cross-host transport and are left out.

## Messaging topologies the design borrows from

### NATS: full mesh inside a cluster, trees for leaf nodes

[nats-io/nats.docs](https://github.com/nats-io/nats.docs), read at `f115bec` (2026-08-24).

- **Mesh with a one-hop limit.** Cluster servers gossip and connect to every server they learn
  of, "dynamically forming a full mesh"; "NATS clustered servers have a forwarding limit of one
  hop ... Messages received from a route will only be distributed to local clients"
  (`running-a-nats-service/configuration/clustering/README.md:7-9`). Loops are impossible by
  that rule, not by detection.
- **Gateways reduce the mesh.** Gateways "don't form a full mesh between gateway nodes, but form a
  full mesh between clusters instead"; for three clusters of five nodes, 105 full-mesh
  connections against 30 gateway connections (`configuration/gateways/README.md:5-15, 44-53`).
- **Leaf nodes dial out and form trees.** "Unlike cluster or gateway nodes, leaf nodes do not need
  to be reachable themselves and can be used to explicitly configure any acyclic graph
  topologies"; a remote's `hub: true` reverses which side initiates
  (`configuration/leafnodes/README.md`, `leafnode_conf.md:156`). JetStream on a leaf keeps
  working while the hub is unreachable (`jetstream_leafnodes.md:9`).
- **Deduplication by message ID within a window.** JetStream ignores a publish whose `Nats-Msg-Id`
  it has seen within the stream's `DuplicateWindow` ("we only consult the message ID not the
  body"; `using-nats/jetstream/model_deep_dive.md:29-46`; `nats-concepts/jetstream/streams.md:39`).

**Implication.** The design takes three things: spokes dial out to the hub as leaf nodes do (the
pull link of 10.3), a hard one-relay limit instead of loop detection, and dedupe by ID alone. The
design's seen-set outlives every copy's expiry (9.2) rather than using a fixed window.

### MQTT bridges (Mosquitto)

[eclipse-mosquitto/mosquitto](https://github.com/eclipse-mosquitto/mosquitto), `man/mosquitto.conf.5.xml`
read at `83f2620` (2026-10-04).

- A bridge is a broker that connects to another broker as a client and forwards configured
  topics. Loops are the operator's problem: "Care must be taken to ensure that loops are not
  created with this option ... each broker is forever forwarding each other the same messages"
  (lines 2574-2578). `try_private` tells the remote broker it is a bridge so "loop detection will
  be more effective" (lines 2600-2612).

**Implication.** Configured forwarding between brokers, with loop avoidance by care, is what a
general relay mesh turns into. The design forbids relaying a relayed envelope instead.

### Matrix federation

[matrix-org/matrix-spec](https://github.com/matrix-org/matrix-spec), read at `e561a08` (2026-10-23).

- Servers push to each other with `PUT /_matrix/federation/v1/send/{txnId}`. "A Transaction is
  meaningful only to the pair of homeservers that exchanged it" (`content/server-server-api.md:413-420`),
  and "The sending server must wait and retry for a 200 OK response before sending a transaction
  with a different `txnId` to the receiving server" (`data/api/server-server/transactions.yaml:22-28`).
  Events are sent directly to every server in the room, without relaying through a third server.

**Implication.** Matrix pairs a per-hop idempotency key (the transaction) with an end-to-end ID
(the event), and does not move on while a hop's outcome is unknown. Those are the design's
custody receipt per hop, its end-to-end message ID, and its rule against moving to a second
contact after ambiguity.

### Tailscale DERP

[tailscale/tailscale](https://github.com/tailscale/tailscale), `derp/derp.go` read at `566f89e`
(2026-10-24).

- "DERP routes packets to clients using curve25519 keys as addresses ... to proxy encrypted
  WireGuard packets ... when a direct path cannot be found or opened. DERP is a last resort"
  (`derp/derp.go:4-12`).

**Implication.** A relay that carries only ciphertext and is used only when a direct path fails
is a transport-level relay, like ssh's `ProxyJump`. It adds no message semantics and holds
nothing after a connection drops.

### OpenSSH `ProxyJump`, `ControlMaster`, and restricted keys

Local manual pages of OpenSSH 8.7p1, read 2026-10-25.

- `ProxyJump` makes ssh connect "to the target host by first making a ssh(1) connection to the
  specified ProxyJump host and then establishing a TCP forwarding to the ultimate target from
  there" (`ssh_config(5)`). The session to the target is end to end: the jump host forwards
  encrypted bytes and authenticates only the hop to itself.
- `ControlMaster` with `ControlPersist` keeps a master connection "open in the background (waiting
  for future client connections)" (`ssh_config(5)`).
- In `authorized_keys`, `restrict` disables "port, agent and X11 forwarding, as well as disabling
  PTY allocation"; `port-forwarding` re-enables forwarding after `restrict`; `permitopen="host:port"`
  limits `-L` forwarding to that destination (`sshd(8)`). It does not limit `-R` listeners
  (that is `permitlisten`), so a forwarding grant on a hub opens more than one destination; by
  recalled behaviour unix-socket forwarding is not limited by `permitopen` either.

**Implication.** Through a hub, ssh can relay without the hub reading or storing anything, if the
destination accepts ssh from the hub and both are up at once. The design offers that as a route
option (`jump`), but carries it through the hub key's forced command (a `connect` verb used as
`ProxyCommand`) rather than a forwarding grant, so the hub's key stays `restrict`ed; it uses the
store-and-forward relay only where a jump cannot work.

### Store and forward: SMTP, UUCP and netnews

Recalled, not re-read (see the note at the top).

- **SMTP** (RFC 5321). A server that answers the end of the data with success has accepted
  responsibility for delivering or reporting (section 6.1), and a connection lost after the data
  but before that answer can produce a duplicate, which SMTP accepts rather than lose mail. Loops
  are detected by counting `Received:` trace fields, with a suggested limit of at least 100
  (section 6.3). Delivery status notifications (RFC 3464) report per recipient, back to the
  envelope sender.
- **UUCP mail** routed by explicit bang paths (`host!host!user`), with each hop storing the
  message until its next call; RFC 976 describes the conventions.
- **Netnews** (RFC 5536, RFC 5537). Each relay prepends itself to `Path:`, does not offer an
  article to a site already in its path, and refuses an article whose `Message-ID` is in its
  history; so a flood across many paths delivers once per site.

**Implication.** Custody is a per-hop promise, not delivery ("accepted responsibility"), and must
be reported separately from the end-to-end outcome. A trace of hops travels with the message. An
end-to-end ID checked against a durable history at the receiver is what makes several paths
safe. The design keeps all three and drops what this setting does not need: source routing,
flooding and long hop counts.

## What `agent-teams` takes, and where it differs

- **Topology:** a star of persistent links that spokes dial out to (AutoGen, Gas Town's proxy,
  NATS leaf nodes), not a pairwise mesh (Ruflo) and not a single store every agent depends on
  (MCP Agent Mail, Gas Town's database).
- **Relay:** at most one relay per message (NATS's one-hop rule), with a durable spool at the hub
  (SMTP custody) rather than an in-memory forwarder (AutoGen); a transport-only option
  (`ProxyJump`, DERP) where it suffices.
- **Exactly once:** an end-to-end ID with a durable seen-set at the final receiver only (netnews
  history, JetStream's `Nats-Msg-Id`), per-hop custody receipts (Matrix transactions, SMTP's
  accepted responsibility), and no move to a second contact while a hop's outcome is unknown
  (in the spirit of Matrix's retry rule; resends to the same receiver stay allowed, because its
  seen-set absorbs them). Unlike A2A, dedupe at the receiver is mandatory.
- **Authority:** only the final receiving host verifies the coordinator and applies policy; a hub
  forwards and never decides. Credentials name the peer, as Gas Town's proxy certificates do.
- **What none of them has:** the final hop is a terminal composer that only one local program may
  type into, and only when the agent behind it is idle.

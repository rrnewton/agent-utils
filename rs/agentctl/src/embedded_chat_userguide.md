# agentctl chat — durable event-driven coordinator bridge

`agentctl chat` connects an already registered interactive agent to a chat
subscription plugin. The plugin supplies normalized inbound events through the
provider-neutral `chat-subscription` traits. A separately configured one-shot
command performs replies and reactions. Neither executable obtains authority
from its manifest: the operator chooses routing, sender allowlists, environment
names, and the exact outbound helper path.

The bridge admits each provider batch and its replay cursor durably before it
acknowledges that batch. It writes a bounded `prepared` receipt first, calls the
plugin commit callback, waits for the exact matching `Committed` frame, and only
then atomically replaces that slot with a `committed` receipt containing the
delivery ID, provider sequence, host batch sequence, cursor, event count, and
timestamp. A `prepared` receipt is never reported as confirmed. This artifact
proves that the local plugin callback was released and returned exact protocol
confirmation; it does not prove a later provider network or server-side action.
The bridge then sends the request to the configured named agent.
Replies use the exact `CHAT_REPLY` fence supplied in the prompt and are retained
with one stable outbound UUID before any send attempt. A restart retries the same
operation identity; an unknown transport result is never converted into success.

## Configuration

Create a private owner-only JSON file (normally mode `0600`):

```json
{
  "subscription_plugin": "provider-events",
  "subscription_environment": ["PROVIDER_CERT", "PROVIDER_KEY"],
  "channel_ids": ["spaces/example"],
  "allowed_senders": ["users/owner"],
  "agent_name": "coordinator",
  "agent_label": "codex coordinator",
  "outbound_enabled": true,
  "ack_reaction": "🤖",
  "backend_configuration": {
    "schema": "provider.example/v1",
    "data": {}
  },
  "outbound_command": {
    "executable": "/absolute/path/to/reply-helper",
    "arguments": [],
    "environment": ["PROVIDER_ACCOUNT"],
    "timeout_millis": 30000,
    "shutdown_grace_millis": 2000
  }
}
```

The subscription plugin is discovered below `$AGENTCTL_HOME/plugins` (default
`~/.agentctl/plugins`) using its private manifest. The bridge revalidates and
pins the manifest directory and executable before every generation. Provider
credentials do not belong in the manifest or saved bridge state. When a plugin
needs credential paths or tokens from the service environment, list only their
variable names in `subscription_environment`. Names must be unique shell
identifiers, at most 128 bytes each, with no more than 64 names. Every named
value must be present before the plugin can be launched. The host clears the
plugin environment, restores its small documented non-secret baseline, then
adds exactly these operator-selected names. Values are neither serialized nor
reported by `chat status`; a plugin manifest has no authority to add names.

The outbound helper receives exactly one newline-terminated JSON request and
must emit exactly one newline-terminated JSON response. It is started inside the
same reviewed pidfd/private-process-group supervisor used for subscription
plugins, with a cleared environment plus only the configured names. The host
sets `AGENTCTL_PROCESS_SUPERVISED=1`. The helper must not create another process
group. Its operation deadline is positive and at most 30 seconds. At service
startup the host validates and hashes a native helper of at most 64 MiB, copies
those exact bytes into a sealed in-memory executable, and retains that image for
the generation. Reply and reaction operations neither reopen nor rehash the
source path.

If outbound messages use an allowed sender identity, prevent their labelled text
from becoming new requests with a runtime-only exclusion, for example
`chat run --bridge-state PATH --ignore-text-prefix '[assistant'`. Repeat the
option for up to 32 prefixes. Matching is literal and case-sensitive after
leading Unicode whitespace; each prefix must be nonempty, at most 256 UTF-8
bytes, and contain no control characters. The default excludes nothing.
Ignored messages still participate in the original provider batch fingerprint,
replay boundary, and cursor commit, but create no request, reaction ACK or pane
delivery. This does not suppress already admitted work. Supply the options on
every run; they do not change the persisted configuration or apply to `chat tick`.

In `chat run`, a generation-owned worker queues reaction ACKs as soon as the
inbound batch is durably admitted and its provider commit is confirmed. Pane
delivery, later intake, output capture and replies do not wait for that worker.
It runs one ACK at a time through a bounded queue; overflow stays in durable
state for recovery. Failed or uncertain ACKs retain their operation ID and wait
at least 60 seconds before an in-process retry. A restart reconciles pending
ACKs using those same IDs. `chat tick` still completes its bounded ACK work
before returning. During service shutdown, queued work remains pending and an
already admitted ACK is owned until bounded completion or uncertain cleanup.

To run intentionally without reactions or replies, set `outbound_enabled` to
`false`, set `ack_reaction` to `null`, and omit `outbound_command`. The delivered
prompt then explicitly identifies the bridge as inbound-only and forbids reply
fences.

## Initialize and run

The named agent must already exist in the selected registry. Every channel in
`channel_ids`, and every thread within it, routes to that one agent. To give
several agents their own conversations, give each agent its own channel, its
own config, and its own bridge state. Two bridges cannot split one shared
channel by thread. Initialization checks the live pane, plugin installation,
outbound executable, and complete configuration before it creates state:

```sh
chmod 600 chat.json
agentctl --registry /work/project/.agentctl chat init \
  --config chat.json \
  --bridge-state /home/me/.local/state/agentctl/project-chat

agentctl --registry /work/project/.agentctl chat run \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

`run` owns an exclusive state lease. Provider intake blocks on the plugin event
stream; it does not poll a REST listing. Terminal reply capture blocks on
Herdr's `events.subscribe` socket with bounded groups of current closing-fence
IDs and a generic predicate for unavailable IDs. The current groups rearm when
reply routes change, so a consumed closing fence left on screen cannot mask the
next reply. A closing fence under an earlier ID of an open request routes to
that request too, because the block may hold new text. Only the generic
predicate matches such an ID, and it stays matched while any closing fence
remains on screen, so such a block is usually captured at the next recovery
scan rather than at once. Exact IDs route through the in-memory durable-state
index; all 2,048 active requests fit within the subscription budget of 128
predicates, 32 KiB per predicate and 96 KiB total. Provider notices and
SIGINT/SIGTERM interrupt that wait through a local wake descriptor. A
disk-backed terminal and delivery reconciliation occurs every 300 seconds by
default and can be changed with `--reconcile-interval`.

A provider `Gap` is a terminal continuity warning in protocol v1. Its reason does not
contain a recoverable range or completeness proof, so the host journals the
incident, keeps the prior safe cursor, sends no commit, cancels that provider
generation, and exits degraded. `chat status` reports `healthy: false` and the
exact unresolved incident; `chat run` refuses automatic reconnect. One later
message or checkpoint does not prove that a missing interval was recovered.

An operator may approve a retry only when the saved boundary is exactly one
committed `Checkpoint`, the gap proposes that same cursor, and independent
provider evidence establishes that retrying that fixed cursor is safe. Stop the
runner and review its backend's explicit retention-boundary recovery policy
first. The host does not interpret a provider's free-text reason as proof.

```sh
agentctl chat retry-checkpoint-gap \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --expected-gap-sha256 "$GAP_SHA256" \
  --expected-checkpoint-sha256 "$CHECKPOINT_SHA256" \
  --expected-configuration-sha256 "$CONFIGURATION_SHA256" \
  --keep-cursor "$COMMITTED_CURSOR" \
  --evidence-file /home/me/private/provider-evidence.json \
  --evidence-sha256 "$EVIDENCE_SHA256"
```

Supply lowercase SHA256 digests of the exact reviewed `gap.json`,
`checkpoint.json`, `bridge.json`, and evidence file bytes. Evidence must be a
private nonempty JSON object of at most 64 KiB. It is retained as an explicit
operator attestation, not a host-verified claim about provider history. The
command verifies the stopped runner lease, all local digests, and the exact
committed checkpoint receipt; it refuses message-bearing boundaries, including
messages excluded by a runtime prefix filter.

Approval writes only a bounded audit under `gap-retries/`; it preserves the
cursor, request bytes, UUIDs, and unresolved diagnostic. Status reports
`gap_retry_approved: true` and `healthy: false`. The next generation may admit
only the identical checkpoint at the kept cursor. A new exact provider commit
resolves the gap and clears reconciliation; an interrupted resolution is
repaired from that new committed receipt. A new gap revokes the approval before
it is published. Audits retain original state and evidence without eviction,
with a limit of 64 records and 512 KiB per record. This procedure cannot reset
to the provider head, accept loss, replay quarantined work, or claim recovery
before provider confirmation.

For a committed boundary containing messages, use the separate explicit
`chat retry-boundary-gap` operation only after reviewing evidence that the
provider can replay the **exact original batch** at the saved cursor:

```sh
agentctl chat retry-boundary-gap \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --expected-gap-sha256 "$GAP_SHA256" \
  --expected-checkpoint-sha256 "$CHECKPOINT_SHA256" \
  --expected-configuration-sha256 "$CONFIGURATION_SHA256" \
  --keep-cursor "$COMMITTED_CURSOR" \
  --evidence-file /home/me/private/provider-evidence.json \
  --evidence-sha256 "$EVIDENCE_SHA256"
```

It applies the same stopped-runner, digest, evidence and committed-receipt
checks. The audit additionally records exact-boundary authority. The complete
ordered event fingerprint, event count and every retained message guard must
remain equal, including original full provider payloads. Matching only a
message ID or text is insufficient; current REST metadata may differ from the
original event. A checkpoint cannot replace a message-bearing boundary, even
when a runtime prefix filter excluded its message from actionable work.

Approval does not fetch messages, change the cursor, clear the gap, reset to
provider head, or accept loss. The next generation must replay the original
inclusive boundary; any mismatch stops without provider acknowledgement.
Only a newer exact committed replay resolves the incident. Same-cursor replay
creates no new requests or ACK/reply UUIDs, even if the current prefix policy
differs; existing quarantines remain intact. New gaps revoke this authority,
and the same receipt-based crash recovery and bounded audit retention apply.
The checkpoint-only command continues to refuse message-bearing boundaries.
An older host that cannot read an exact-boundary audit fails closed; keep the
matching host available through retry and confirmation.

For a bounded local work recovery pass while the daemon is stopped (`tick` does
not repair provider gaps):

```sh
agentctl --registry /work/project/.agentctl chat tick \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

Inspecting status is provider- and Herdr-free and does not perform recovery
writes:

```sh
agentctl chat status \
  --bridge-state /home/me/.local/state/agentctl/project-chat
```

Inspect one exact active retained request for latency/audit evidence without a
provider, outbound helper, Herdr call, or state write:

```sh
agentctl chat inspect \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --request 64_LOWERCASE_HEX_CHARACTERS
```

The stable output schema is `agentctl-chat-request-inspection/v1`. It contains
the request phase and source IDs; `provenance.host_batch_sequence`,
`provider_sequence`, `delivery_id`, and cursor; `timestamps.admitted_at_millis`,
`delivery_started_at_millis`, `delivered_at_millis`, `ack_started_at_millis`,
and `ack_completed_at_millis`; delivery and acknowledgement receipts; and a
bounded `replies` array with each ordinal, phase, operation ID, provider message
ID, `captured_at_millis`, and `sent_at_millis`. Start timestamps record the first
durable attempt. Delivery, ACK, and reply completion timestamps remain null
after failed or outcome-unknown operations and are written only with positive
Herdr/provider evidence. The command accepts one exact key and reports only an
active retained request. An acceptance harness that later calls `chat close`
must copy and fsync this inspection document first.

After one line saying that the request arrived through the chat bridge, each
request prompt gives the message's `Source:` ID, its `Sender:`, and its
`Thread:` ID, and says whether the message starts a new thread or replies in an
existing one. When a provider payload with schema `google.chat.message.v1`
quotes an earlier message, a `Quoted message:` line names that message and its
text follows with every line prefixed by `> `. A long quote keeps its beginning
and end joined by ` ... `, and the line then states how many characters the
provider's quoted text has. Identifiers stay on one line and control characters
are replaced, so this text cannot break a later terminal capture. In
identifiers and quoted text, the `<` of a reply-marker token such as
`<CHAT_REPLY_` is printed as `‹`, and each character of a run of three or more
backticks or tildes as `ˋ` or `˜`, so no row can form a reply marker or open a
code fence however the terminal wraps a long line. These tokens are found as if
every non-ASCII character and every tab were absent, because a terminal program
may drop a character it draws with no width. The look-alikes count as absent
too, so when replacing one token joins its neighbours into another, as in
`<<CHAT_REPLY_`, that one is replaced as well, and no marker or fence forms
whichever of these characters a terminal drops. The message's own text follows
the front matter as sent. For a reply in an existing thread, the prompt also
prints the exact command that shows the thread's earlier messages. The command
is left out when a word of it cannot be printed on one line or holds such a
token, because a rewritten word would name a different thread or state
directory. The command starts with the absolute path of the service's own
executable, because a service need not have `agentctl` on `PATH`:

```sh
/opt/agentctl/bin/agentctl chat thread \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --thread spaces/example/threads/one --last 10
```

If that executable's file has been deleted, the command names the file now at
the same path, normally the build that replaced it; with no file there, it
starts with plain `agentctl`.

`thread` prints the thread's retained requests and the replies captured for
them, oldest first, each with its UTC time, its age, and whether a reply was
sent, and prefixes every line of message text with `> `, rewriting reply-marker
tokens and fence runs the same way as the prompt. Times come from the bridge
host's clock, and the entries are ordered by them. `--last` selects how many of
the most recent messages to show (default 10, at most 100). The bridge retains
only requests it admitted from allowed senders that have not been retired, so
retired requests, other senders' messages, and anything the bridge never
received are absent; the provider's own thread is the complete record. Like
`inspect`, it reads under the shared state lock and never writes state or
contacts Herdr, a helper, or a provider.

An owner or operator can explicitly publish a new root message through the
configured outbound helper without pretending it is a reply:

```sh
agentctl chat publish \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --channel-id spaces/example \
  --request-id 123e4567-e89b-42d3-a456-426614174000 \
  'Please reply to this bridge test.'
```

`publish` accepts `--file PATH` instead of positional text. It requires an
outbound-enabled state and an exact channel from `channel_ids`, validates a
nonempty body of at most 30,000 UTF-8 bytes, and sends `thread_id: null`. The
lowercase RFC 4122 version-4 UUID is caller-owned; an uncertain result may be
retried only with the identical UUID, channel, and body. The command runs the
helper only through the reviewed process supervisor, binds the returned message
resource to the requested channel, and prints the exact v1 success receipt. It
neither writes bridge state nor creates an event-loop reply route; this is an
explicit operator action, not automatic subscription behavior.

Explicitly close reply capture for an old request:

```sh
agentctl chat close \
  --bridge-state /home/me/.local/state/agentctl/project-chat \
  --request 64_LOWERCASE_HEX_CHARACTERS
```

Closure is the provider-neutral terminal route lifecycle; idle/done status and
the first reply do not close a route because a request may send later progress
updates. A closed request remains active until its prompt delivery is confirmed,
its reaction is disabled or confirmed, and every captured reply is sent. It is
also retained while it belongs to the current inclusive replay cursor. Once all
conditions hold and a later cursor is durable, the bridge fsyncs a retirement
journal containing the exact reaction and reply operation/provider receipts,
installs a compact replay and closed-nonce guard, and only then deletes the
active request/reply files. Startup completes an interrupted retirement.

The active population remains capped at 2,048 requests. The compact route and
replay ring retains the latest 4,096 retired identities, matching the pane
marker bound, and the parallel retirement audit ring has 4,096 slots of at most
256 KiB each. Older replay safety relies on the subscription contract's
inclusive monotone cursor; a cursor regression or a reused message identity
with different content fails closed. This is bounded local retention, not a
claim of infinite local audit history.

Reply markers count only on rows that can be the agent's own output. A terminal
agent draws each prompt it receives, and the output of each tool call it makes,
as an item of its own, and the rows of those items are skipped, so a reply block
that another agent quotes in a message to this one, or that a tool prints from a
file, is not posted, except in a read of an idle Claude Code pane longer than
its screen, as described below. It is not reported either, unless the first row
of that item is out of view, the item is a Claude Code prompt whose first row is
the first nonblank row of the read, or the read is such a longer read, as
described below. A prompt starts at a row whose text after its indentation is
`❯`, `›`, `↳` or `»`, alone or followed by a space or a no-break space, and its
wrapped rows continue two or more columns further right. Claude Code draws a
prompt and its input box after `❯`; Codex draws a prompt after `›`, a prompt it
holds until a running tool call ends and a hook's notice after `↳`, and its
input box after `»`. Tool output starts at a row whose text after its
indentation begins with `⎿`, or is `└` alone or followed by a space, and its
later rows continue right of that character. Codex draws `└` alone when the
output's first line is empty. A code fence in tool output neither opens nor
closes a fence.

Inside an open block these characters are reply text, such as a tree drawn with
`└`, except on a row that starts a new item: a row whose text starts with a
prompt character or a bullet (`•`, `⏺` or `●`) left of the column where the
block's opening marker starts, or with `⎿` at or left of that column. Claude
Code's compact view draws a tool call as a row at the margin of the message,
with no bullet, and its output after `⎿` at the same column. The block then ends
unfinished and is handled as a partial block, as described below, unless the new
item is a bullet row that holds the block's closing marker. Each request prompt
therefore asks the agent to start a message with the opening line and write the
whole block in that message, with no tool call inside it.

These rules read how a row starts, not who wrote it, so they can fail both ways.
A row of the agent's own text outside a block that starts like a prompt hides
the rows two or more columns right of it, and one that starts like tool output
hides the rows right of its first character, so a block in those rows is not
read. A tool call that the compact view draws as a single row, with no `⎿` row
under it, reads as text, so inside a block it is posted as part of the reply.
An item that starts with none of these characters can show text the agent did
not write: Codex's `Goal active` notice shows the goal the agent was given,
wrapped to the left edge, and a reply block quoted in that goal is read as the
agent's own.

The bridge reads only the rows on the screen of a pane that herdr reports keeps
no scrollback, one whose `scroll.max_offset_from_bottom` is 0, such as a Claude
Code pane, for the reason given in the next paragraph. For any other pane, such
as a Codex pane, it asks herdr for the newest 4,000 lines with each wrapped line
joined into one, as herdr reads the pane for an output event; herdr returns at
most 1,000 lines for either read. If that read returns no text, the bridge asks
for the rows as the terminal draws them instead. It looks the pane up again
before each read, so a pane that starts or stops keeping scrollback between the
lookup and the read, as a program does when it enters or leaves the terminal's
alternate screen, is read the old way once, and for a pane that has just entered
the alternate screen, that one read can be the joined read described below. A
`scroll` value that is not an object of those three counts fails every lookup
that reads that pane, for prompt delivery as well as for reads, rather than let
the bridge guess. A lookup by session reads every pane, so one pane's malformed
value fails every lookup by session. The rows above the first nonblank row that
starts at the left edge continue an item whose first row is out of view, which
can be a prompt or tool output, so an opening marker there opens no block,
although an unavailable ID in it is still reported, and a code fence that opens
there ends at that row. Once a prompt has scrolled off the top of the screen,
Claude Code can draw a copy of it over the screen's top row, at the left edge
and cut to one row, above rows of whatever item the screen starts in. So a `❯`
row at the left edge that is the first nonblank row of a capture is skipped: it
starts no prompt, and it is not the first row at the left edge. A Claude Code
prompt that really starts at the top of a capture is then read as rows above the
first row at the left edge, so a block it quotes is not posted, but it is
reported unless its text is the end of a stored reply. Codex draws no such copy,
so a Codex prompt row at the top of a capture starts a prompt, as it does
anywhere else. A block is therefore posted only by a capture that shows both the
first row of the message that holds it and the whole block. A block that starts
its message needs only the whole block in view; a block after other text in its
message also needs the message's first row, which leaves the screen sooner. So
from a Claude Code pane a block is not posted if it is taller than the screen,
or if its closing marker is more than a screen below the first row of its
message, as in a long message that ends with the block; a recovery scan that
sees its closing marker reports it instead, as described below. A capture that
sees only part of a block handles that part as a partial block, as described
below, except in the longer reads described next.

A Claude Code pane keeps no scrollback, yet a read of more lines than its screen
has rows can return more. Herdr 0.8.0 builds such a read of an agent that is
idle and takes mouse wheel input: it scrolls the agent's view up with wheel
events, joins the screens it sees where they overlap, and scrolls the view back
down. That can take up to 20 seconds, and anyone watching the pane sees the view
move. The join keeps each row that differs from the row at the same position on
the screen before, so each time the copy of a prompt that Claude Code draws over
the top row changes, the copy lands between rows of the conversation. That is
why the bridge reads only the screen of a pane that keeps no scrollback. Herdr's
output subscriptions never scroll the view, so an output event never carries
such a read, but the bridge's own reads of a pane still get one when herdr does
not report whether the pane keeps scrollback, and can get one once when the pane
enters the alternate screen between the bridge's lookup and its read. The extra
rows come before the screen's rows, so in every such read they decide how the
screen's top rows are read, and the screen's own pinned row is no longer the
first row of the capture: it starts a prompt, which hides the agent's rows under
it. These reads can mislead the rules above both ways. Rows of a prompt whose
first row is out of view can follow a row of the agent's message, so a block
quoted in that prompt is posted as the agent's reply. The agent's own rows can
follow a pinned prompt row, a stale prompt row, or a tool output row. Where they
are indented far enough to read as that item's rows, two or more columns right
of the prompt character or right of the `⎿` or `└`, a block in them is skipped,
neither posted nor reported, and a block of the agent's that such a row lands
inside is reported as having no closing marker, although the agent wrote one. A
later read that shows only the screen reads the block by the rules above, if it
is still in view: it is posted if that read also shows the first row of its
message, and otherwise reported, unless it was already reported.

Reading only the screen has a cost. For a Claude Code pane, the bridge's reads
when it starts, at each reconciliation, when the pane settles idle or done, and
after an output event names an unknown reply ID see only the screen. An output
event carries herdr's read of the pane taken when the matched line appears,
which for such a pane is its screen, so a block that fits on the screen is
normally caught when its closing marker appears. If that event is missed, for
example while the bridge waits to subscribe again after losing its connection to
herdr, or while it restarts, and the block leaves the screen before the bridge's
next read, the block is neither posted nor reported. A joined read could have
recovered it. Before herdr 0.8.0 added those reads, a read of such a pane
returned only its screen, so the bridge lost such a block then too. The bridge
accepts that loss rather than scan rows joined from different times, which can
post a block the agent did not write. A block that the screen cannot show whole
together with the first row of its message, as described above, can be lost even
when no event is missed: only recovery scans report a partial block, and during
a busy turn a recovery scan normally comes only when the pane settles, so if the
agent writes more than a screen after such a block before then, the block is
neither posted nor reported.

Each request's prompt gives one reply ID, a short number, for every reply to
that request: `001`, `002`, and so on, counting across all requests and past
`999` with more digits. The prompt asks the agent to use that same ID for each
reply and not to increment it. The bridge assigns a number when it first writes
a prompt that shows it, and never assigns it again: the next number is kept in
`reply-aliases.json` in the state directory and saved before the prompt is
written, so a restart, a crash, or a rollback to an earlier release and back
does not reuse one. A number that the agent writes in a marker line before the
bridge assigns it, such as a guessed or example ID, is reported like any other
unavailable ID and skipped when assignment reaches it; up to 1,024 such numbers
are kept, the lowest ones. The number of a closed request stays recognized as
long as that request's long ID does, so a late block under it is ignored rather
than reported. An earlier release reports a short ID as belonging to no request.

Just before it writes a prompt that gives a request its number, the bridge reads
the agent's pane, as described above, and skips every number that the read shows
written as a short reply ID anywhere: in a block, in running text, or in a
prompt or tool output the agent received. These count toward the 1,024 skipped
numbers kept for later requests, and the request whose prompt follows the read
gets none of them, however many the read shows. So a block already in the pane
when a request gets its number is never posted as that request's reply; where a
capture reads it, it is reported as naming no open request. If the pane cannot
be read, the prompt is not written and the request waits for the bridge's next
attempt. A block that the read does not show can still be posted to a request
that gets its number afterwards, if a later capture sees it: one that comes back
into view, as when someone scrolls a Claude Code pane's view up, and one in an
output event that herdr read before the block left the view but the bridge
handles only after the prompt. Once a request's prompt has reached the agent, a
block under its number is posted to it, even if the agent meant it for another
request, as a block under a long ID is.

A request gets its number when its prompt is written, but the prompt can then
wait in the agent's queue, for example while the agent is busy, and the agent
cannot know the number before the prompt is typed. A block under that number
before then was written for another request, by an agent that guessed the next
number or copied an example. While the queue holds the prompt in its inbox, or
does not hold it at all, the bridge posts no block it reads under that number
and remembers the text of each complete one in `reply-aliases.json`, so no block
with that text is posted to the request after its prompt is typed either. That
text is compared as a stored reply's is, as described below, so a paragraph the
terminal re-wraps or a border it redraws still matches, but a table redrawn in
one of the ways listed there can read as new text. A block with new text under
the number is posted then. Up to 256 remembered blocks are kept, the newest.
Each delivery, of a request's prompt or of a routing-error prompt, can type
every prompt waiting in the queue, so before it types any, if a request whose
prompt has not been typed has a number, the bridge reads the pane and remembers
in the same way the blocks the read shows under such numbers. While
`reply-aliases.json` cannot be read, it reads the pane too if any request's
prompt has not been typed, but then remembers nothing, as described below. If
the pane cannot be read then, or a block it shows cannot be written to
`reply-aliases.json`, the bridge types nothing and tries again later.
When a recovery scan reads a block that is not posted for this reason, the agent
gets a routing-error prompt that says the block was not sent, why, and that no
block with its text will be sent to that request, and lists the open requests.
It names the request the block was probably meant for in the same case as for a
block under an unavailable ID, described below. While a request's queue entry
cannot be read, a block under its number is held: it is not posted, remembered,
or reported, and a later capture decides.

These rules see only the bridge's own queue, and they have gaps. A prompt typed
into a Claude Code session that is still working waits in that program's own
queue, which the bridge cannot see, so a block under its number written then is
posted. A block that first appears after the read before typing is posted unless
a capture reads it before the prompt is typed, and a nonzero `--ready-timeout`
widens that window, because the drain then waits for the agent before it types.
A block written while the prompt waits that no capture reads, and that is out of
view at the read before typing, is posted if a later capture sees it: one that
comes back into view, as when someone scrolls a Claude Code pane's view up, and
one in an output event that herdr read before the block left the view but the
bridge handles only after the prompt is typed. A remembered table that the
terminal redraws after the prompt is typed, in one of the ways listed below, can
read as new text and is then posted. Each rendering of a table whose cells wrap
differently is also reported as a block of its own, so it can bring the agent
one more routing-error prompt. Other commands that type queued prompts do so
without that read: `agentctl send`, `agentctl drain`, and `agentctl goal` given
a goal, and the same three commands of the older `herdr-agent`. A request whose
queue entry shows that typing its prompt started counts as reached, even if the
typing then failed. While a request's queue entry cannot be read, the read
before typing does not remember blocks under its number, so if its prompt was in
fact waiting and is typed, a block under its number that is still in view once
the entry can be read again is posted. A remembered block is forgotten once 256
newer ones are remembered. While `reply-aliases.json` cannot be read, nothing is
remembered, so once the file is repaired, a block still in view under the number
of a request whose prompt was typed in that time is posted to it. Only a
recovery scan reports such a block: one that the read before typing or the
capture after an output event remembers is reported only if a recovery scan
reads it too.

A request prompted before short reply IDs, and every request prompted while
`reply-aliases.json` cannot be read, gets a long reply ID instead: the request's
nonce, an underscore, and a number from 1 to 999999 with no leading zero, which
the agent increments for each reply. Long IDs stay accepted for every request.
While the file cannot be read, the service logs that it is unusable, `chat tick`
reports that among its notes, and `chat status` shows it in
`reply_alias_problem`, which is otherwise null; blocks under short IDs are
reported as naming no open chat request, and the file is left as it is for an
operator to repair. Deleting it starts the numbering again at `001`, as a new
state directory for the same pane does, and replacing it with an older copy
starts it again at that copy's next number. The read before each prompt still
skips the numbers the pane shows, but a block under an old number that is out of
view then can reach the new request.

Herdr reports an output pattern only when it starts to match, and a request
keeps its short ID for every reply, so while the closing line of one reply is in
view, the closing line of the next raises no event. While a closing line under
an open request's short ID is in view, the service therefore reads the pane
itself every 2 seconds, until no such line is left. In view means in the
bridge's read, which is herdr's window for output events too: the screen of a
Claude Code pane, and the newest 1,000 lines of a Codex pane. Each of these
reads takes the lock that prompt delivery takes, looks the pane up in herdr, and
reads it, up to 1,000 lines of a Codex pane, but saves no snapshot. A capture of
the read follows for each open request whose closing line it shows, as for an
output event, and reads that request's queue entry too when the bridge has not
seen its prompt typed and either the request has a number or
`reply-aliases.json` cannot be read. A closing line inside a prompt or tool
output the agent received keeps them going too, although it is never posted. A
read that fails is logged once and retried every 2 seconds, and its first
success after that is logged as well.

Replies are recognized by their text, not by the number in their reply ID. When
a block appears under any reply ID of an open request, the bridge compares its
text with the replies it has already stored for that request. The comparison
ignores whitespace and the box-drawing characters, U+2500 to U+257F, that a
terminal draws for table borders, so a paragraph the terminal re-wraps or a
border it redraws at another width still matches. Every other character counts,
including the block elements of a progress bar. A block whose text matches a
stored reply is not posted again; any other block is stored as the request's
next reply and posted. So an answer the agent sends twice to one request, under
one ID or under two, is posted once, and two identical short replies to one
request, such as two `ok` progress notes, are posted as one. The same text sent
to two requests is posted to each.

A table cell that wraps onto several lines at one width and fits on one line at
another changes the order of the characters, so tables are also compared column
by column. A row line starts, ends, and divides its cells with `│`; a table
drawn with other characters, such as `|` or `┃`, is always read in row order.
Adjacent row lines are read column by column, as one row, only when a
word-wrapping renderer could have drawn them from one row: every line has the
same cell widths and a space at each side of each cell; each column's text sits
at the top of the lines or is centred in them, with the odd line below; the
text of at least one column fills the lines; and each line break was needed
because the next word would not have fit. A renderer that keeps the space of a
line break, such as Claude Code's, starts the line after an exactly full one
with that space, or puts the space on a line of its own when the next word
fills a whole line, and the space counts toward its line. Widths are counted in
terminal columns, as a renderer pads cells: an emoji or a CJK character takes
two columns, and a nonspacing mark such as a combining accent none. That match
counts only when the stored reply's tables have other cell widths, since at the
same widths a renderer wraps the same text the same way. The block is then
skipped and logged, as described below. It is logged because one case reads the
same without being a redraw: a table with no rule between its rows, drawn at
other widths, whose rows all pass those tests and whose text differs from a
stored reply only in how it divides into rows or where a space falls inside a
cell.

A table redrawn at another width can still be posted again. Adjacent row lines
with the same number of cells are read as one group, which ends at any other
line, such as a rule. A group that fails the tests above is read in row order,
so a redraw that wraps it differently reads as new text. That can happen in
four cases; a renderer that lays out tables unlike Claude Code, such as by
centring text with the odd line above or drawing cells without a space at each
side, can cause others:

- a table with no rule between its rows, where one group holds several rows.
  Claude Code draws a rule between every two rows, so this needs another
  renderer;
- a cell holding a character that the agent's renderer measures at another
  width than the bridge does, such as some emoji sequences and most spacing
  vowel signs of Indic scripts;
- a cell holding a run of two or more spaces, or words joined by a no-break
  space or a tab. A renderer can leave the extra spaces at the end of a line,
  where they look like padding, and need not break a line at a no-break space
  or a tab, so the bridge cannot tell that a break was needed;
- a redraw that switches between a grid and one `Header: value` line per cell,
  which a renderer can draw when a cell would need too many lines or the grid
  would not fit the terminal.

A block that cannot be posted is skipped, and the other blocks are still
captured. That covers a block that is empty, holds terminal control characters,
or exceeds 30,000 UTF-8 bytes once the agent label is added, a block beyond the
request's reply limits, and a block that matches a stored reply only column by
column. `chat run` logs such a block as
`TIME agentctl: chat reply capture: reply block ID (text HASH) was not sent: REASON`,
where TIME is the UTC time that begins every `chat run` log line and HASH is the
first 12 hex digits of the SHA-256 of the block's text without whitespace or
box-drawing characters. A process writes each distinct line once while it
remembers it; it remembers the latest 4,096 distinct lines, and writes a
forgotten one again. One capture pass logs at most 128 such lines, so on a
screen with more refused blocks than that, the rest are not logged while they
stay visible. The agent is not told. Nothing the agent prints stops the bridge:
a snapshot larger than 2 MiB is read from its newest complete lines, with one
log line, and a snapshot with more than 4,096 reply blocks is read from its
newest 4,096, with one log line from a recovery scan. Only state, file system,
and provider faults are errors. A reply reminder check, described below, is the
exception: it logs a fault of its own, such as an unreadable reminder record,
and goes on without reminding.

A reply marker in the pane whose ID is not available, such as a typo or a stale
block left in scrollback by another bridge state, produces one routing-error
prompt to the agent. An ID is unavailable when its number is malformed or it
names no request the bridge knows. A well-formed ID of a closed request is
ignored. The prompt is never posted to chat. It says that the block marked with
the unavailable ID was not sent, and then lists the open requests, each by the
ID of its next reply, as they stood when the prompt was written. First come
those with no reply yet, most recent first, each with the time since the bridge
admitted it, such as `10m ago`, because a mistyped ID was most likely meant for
one of those. Then come those that already have a reply. The first list names up
to 32 requests and the second up to 10, and each counts the rest, as in
`... (+37 more)`. When every listed request has a reply, or none is listed, the
prompt says so. It ends by asking the agent to send the block again with the ID
of the request it answers. An open request is listed once delivery of its prompt
to the agent is confirmed or uncertain, and before that while the agent's queue
holds the prompt, as it does when a submission failed after queuing it and waits
for a retry, since that prompt reaches the agent through the same queue as the
routing-error prompt. A request whose prompt has not reached the queue, because
the bridge has not yet submitted it or its submission was cancelled or failed
before queuing, is not listed. Only a prompt that reports an unavailable ID
looks for request prompts in the queue. A request whose record there cannot be
read is listed too, since its prompt may have reached the agent, and the
routing-error prompt is sent all the same.

The prompt says that the block was probably meant for a request when that
request is the only listed one with no reply yet that the agent may have seen
when it wrote the block, and its prompt is known to have left the queue's inbox,
where a prompt waits until the queue types it into the pane: its delivery is
confirmed or uncertain, or the queue reports it past the inbox. A request whose
prompt still waits in the inbox is listed but does not count, since the agent
had not seen it when it wrote the block. A request whose queue record cannot be
read counts, since the agent may have seen it, but is never named, so while one
with no reply yet is listed, no request is named. The bridge cannot tell when
the block was written, so a request whose prompt left the inbox after the agent
wrote the block, but before the bridge reported it, can still be named. The
exception is a request whose prompt reaches the agent when the bridge delivers
an earlier routing-error prompt still waiting in the queue, which it does before
writing a new one: the open requests are taken before that delivery, so such a
request is listed but not named. When the prompt names a request, it also asks
the agent not to send a block it did not write, such as one quoted in a message
it received, since the agent could otherwise send such a block under that
request's ID.

A recovery scan also reports a partial block of an open request: an opening
marker with no closing one after it, or an unopened block, which ends in a
closing marker in a message that shows no opening one before it or whose first
row is out of view. It does not report a part that holds no text or is the start
or the end of a reply already stored for that request, read in row order or
column by column. A part cut inside a table row that wrapped at another width
than the stored reply's is still reported, once. The prompt says what the
capture was missing: the start of the message that holds the block or an opening
line for the block, or the block's closing line. A block that a prompt or a tool
call interrupts reads as both: the rows before the new item as a block with no
closing line, and the rows after it, up to the closing line, as an unopened
block. The prompt then asks the agent to send the block again, starting a
message with its opening line and writing the whole block in that message, with
no tool call inside it, and not to send a block it did not write, such as one
quoted in a message it received. A block another agent quoted in a message whose
first row is out of view, including a Claude Code prompt under the copy of its
first row, or in a Claude Code prompt whose first row is the first nonblank row
of the read, and a block a tool printed in output whose first row is out of
view, are reported too, as described above. So, in a read of an idle Claude Code
pane longer than its screen, are a quoted block and the agent's own block with a
copied prompt row or a tool-output row inside it, which reads as a block with no
closing line. When a closing line was missing, it first says that a block still
being written goes out once it is complete, if the screen then shows the first
row of its message and its closing line, since the agent may not have finished
the block when the pane was read. A complete block under that ID is still posted
if its text is new. One prompt can report both unavailable IDs and partial
blocks.

A partial block is identified by its ID, by which end of it was missing, and by
a digest of 64 characters of it, not counting whitespace or box-drawing
characters: the last 64 of an unopened block, and the first 64 of a block with
no closing line. A block with fewer than 64 such characters is identified by all
of them. Those characters stay the same while the block scrolls, so a block
leaving the top of the screen is not reported again while at least 64 characters
of it are in view, and what is left of it after that is not reported if an
unopened block under its ID was already reported. The rows of an unopened block
begin no higher than the last row above its closing line that starts at the left
edge with a bullet (`•`, `⏺` or `●`) and a space, as the first row of a message
does, so while that row is in view, rows of an earlier item are not counted,
even once the prompt or tool output row that started that item has left the
screen or reads as Claude Code's copy of a prompt. Two different blocks under
one ID are each reported. A partial block that only scrolls up the screen is
therefore reported once. It can be reported again when a later recovery scan
shows it differently, such as a block with no closing line and fewer than 64
such characters that has grown; a block reported with no closing line while it
was still being written, which is reported as an unopened block once a recovery
scan sees it complete but not the first row of its message; and a block first
reported when fewer than 64 such characters of it were in view, at the top of
the capture, once a recovery scan shows the first row of its message or 64 such
characters of it, as it can when rows move back down the screen.

Reported entries are kept in `fence-feedback.json` in the bridge state
directory, so each unavailable ID and each partial block entry is reported at
most once per state directory, including after a restart. An unavailable ID is
kept as the ID itself. A partial block is kept as `ID KIND HASH`: KIND is
`unopened` for an unopened block, `remnant` for fewer than 64 characters of one
at the top of the capture, and `unclosed` for a block with no closing line, and
HASH is the first 12 hex digits of the SHA-256 of the characters its digest
covers. A history written by an earlier release keeps the bare ID of a partial
block, and that ID still covers every partial block under it. A new state
directory starts with no reported entries. A marker that stays visible after its
report is left out of later prompts, and a later block that reuses a reported
unavailable ID is not reported again. A block under an ID that belongs to no
open request is never posted. Once its ID is reported, its only trace is a log
line, and only recovery scans write that line. Recovery scans run when
`chat run` starts, at each `chat tick`, at each reconciliation while the agent
pane is idle or done, when the pane settles idle or done, and after output names
a reply ID that belongs to no request the bridge knows. Each recovery scan that
sees markers or partial blocks already reported logs one
`already reported, so not repeated` line that names up to 8 of them and counts
the rest, up to 128 per scan. Other captures do not write that line, so a reused
ID that leaves the screen before the next recovery scan leaves no trace. Already
reported entries are set aside before the bound on new ones, so a screen full of
old markers cannot hide a new one. While a routing-error prompt is still queued,
newer entries wait for it instead of producing a second prompt. The exact
pending prompt is saved before submission, so recovery settles its original
queue ID even if a crash hides the submission result or newer unavailable
markers appear. A prompt whose queue outcome is uncertain counts as reported: it
is not submitted again, even if it never reached the agent. The history retains
up to 4,096 distinct reported or pending entries. At that limit, new diagnostics
stay held; reported entries are never evicted or submitted again. Deleting
`fence-feedback.json` clears the history, so the entries it held can be reported
once more. If the file cannot be read or is outside its bounds, the error names
it and diagnostics stay held until it is repaired or deleted.

A per-thread post-rate breaker bounds any remaining reply loop. One provider
thread may reserve 8 distinct reply operations within 60 seconds. That leaves
room for several requests in one thread, each with progress updates and a
multi-message answer, and each request prompt states this budget to the agent.
The next reply to that thread trips the breaker. Replies to that thread then
stay captured but unsent for 300 seconds; replies to other threads are
unaffected. Held replies go out in order, under their original operation IDs, at
the first retry after the cooldown ends: the next reconciliation (every 300
seconds by default; see `--reconcile-interval`), the next time the agent pane
settles idle or done, the next captured reply for the same request, a restart of
`chat run`, or `chat tick` while the daemon is stopped. Those sends count
against a new window, so a longer backlog goes out 8 at a time, with a new hold
after each group of 8. The breaker therefore bounds only fast loops. A loop
never trips if each post starts at least 60 seconds after the receipt of the
post 8 before it, so it can post up to 480 times an hour to one thread. A faster
loop trips on its 9th post within 60 seconds; one that posts every 3 seconds
trips 24 seconds in. It then sends 8 more posts after each 300-second hold,
about 96 an hour. While a thread is held, every attempt fails with a
`post-rate breaker` error that names the thread and the release time in
milliseconds after the Unix epoch. `chat run` logs that error, and `chat tick`
reports it and exits with status 75. `chat status` reports the breaker under
`reply_breaker`: its limits (`max_replies_per_thread`, `window_seconds`, and
`cooldown_seconds`), each held thread in `held_threads` with
`held_until_millis`, `held_for_seconds`, and
`recent_or_unresolved_reservations`, and an `error` that is null unless the
record is unusable. Status never changes the record, and it lists only threads
whose trip is still live: a thread whose trip has expired is not listed even if
8 of its reservations are still unresolved, and its next new reply trips the
breaker again. Neither the agent nor the chat thread is told about a hold: a
notice to the agent could prompt more replies, and a notice in the thread would
be one more post to the thread being held.

To release held threads early, first confirm that no reply loop is running, then
delete `reply-breaker.json` from the bridge state directory. This releases every
thread and discards the budget history of all of them; held replies go out at
the next retry. If that file cannot be read or is outside its bounds, replies to
every thread stay held until it is repaired or deleted: each publish error names
the file, and `chat status` shows the failure in `reply_breaker.error`. A
reservation or trip stamped later than the current time, as after the wall clock
steps back, is moved to the current time, so its window or cooldown restarts
once instead of lasting as long as the step. The next send attempt to any thread
saves the moved stamps; until then, `chat status` reports such a trip as held
for a full cooldown from the time of each call. Reservations are durable before
a provider call, including calls whose outcome is unknown. Unresolved
reservations keep their budget until the same operation is reconciled or the
ledger is explicitly reset. A valid receipt starts its 60-second retention
window; retrying an expired completed reservation must pass the current budget
again. A full ledger holds new sends until completed entries expire. This can
conservatively hold replies after a failed attempt.

A Claude Code agent sometimes writes its reply only in its thinking. The bridge
reads only the agent's ordinary text output, so it never sees such a reply, and
nobody is told that the reply was lost. `chat run` therefore reminds a Claude
Code agent, once, of each request it has not answered. Reminders need
`outbound_enabled`, since an inbound-only agent is told not to write reply
blocks, and they read Claude Code's own record of whether a session is busy: the
file `sessions/<pid>.json` in `$CLAUDE_CONFIG_DIR`, or in `$HOME/.claude` when
that variable is unset or empty. The bridge uses four fields of that file,
`pid`, `procStart`, `status`, and `statusUpdatedAt`, ignores the others and
never logs them, and opens no other file in that directory. It uses a record
only while a live process in the agent's pane has the same process id and start
time, the earliest-started such process if there are several. At startup the
service logs the directory it reads, or that reminders are off because the
variable that applies does not name an absolute directory. An agent that is not
a Claude Code session, such as a Codex agent, is never reminded.

Every 15 seconds, starting when the service starts, the service looks for a
request to remind the agent of. Such a request's prompt was confirmed delivered
to the agent after reminders started and no more than 24 hours ago, and the
request is not closed, has no stored reply, even one the post-rate breaker holds
back, and was never reminded. A request whose delivery is pending or uncertain
is never reminded, since the agent may never have seen its prompt. When there is
such a request, or a reminder waits in the queue, the service asks herdr whether
the pane is idle or done and reads the agent's session record. The agent is due
a reminder once that record has said `idle` for 60 seconds and the request's
prompt was delivered before that idle period began. The 60 seconds are set by
reasoning, not measured: the queue types a reminder only when the agent is
ready for input, and one the agent is not ready for in time is taken back out,
as described below, so a settle time that is too short costs at most one early
reminder, never an interrupted turn. The service then reads the pane and
captures the read as a recovery scan does, so a reply on screen that no capture
has stored yet counts. That read can type a routing-error prompt or a request's
prompt into the pane, and Claude Code marks its session record busy only some
time after text is typed, so no reminder follows a read that may have typed
anything or left anything waiting in the queue. Otherwise the service sends the
reminder only if the session has stayed idle since the same moment.

Claude Code 2.1.285 writes `idle` only while the session waits at its prompt
with nothing it started still running: it writes `busy` while it works on a turn
or while a subagent it started runs, `shell` while a command it started in the
background runs, and `waiting` while it waits for its user to answer a question
or grant a permission. The bridge counts only `idle` as idle, and any other
value, those three included, as busy. So an agent that leaves a background
command or a subagent running is not reminded until that ends.

The reminder is one prompt, sent through the same queue as request prompts. It
names the reply ID of each request it reminds, newest first with how long ago
each prompt was delivered, up to 32 of them, and counts the rest. It says that
the bridge has received no reply, that a reply written only in thinking is not
delivered, and how to write a reply block as ordinary text, and that the agent
can ignore it if a request needs no reply, was answered another way, or is still
being worked on. The requests are recorded as reminded in `reply-reminders.json`
in the bridge state directory before the reminder is typed, so a crash or a
restart never reminds them again, and a reminder that may have been typed is
never typed again. A recorded reminder that the queue no longer knows, as when
the agent's queue was replaced, may have been typed, so it is never typed again
either. The service waits for the agent to be ready for the reminder no longer
than `--ready-timeout` or 10 seconds, whichever is shorter. A reminder not typed
by then is taken back out of the queue, so that it is never typed later, after
the agent may have answered, and its requests can be reminded again; the agent
is then due no reminder until its session record next says it went idle, or the
service restarts. If the queue cannot give the reminder back, it stays recorded
and nothing else is composed; a delivery to the agent can still type it, and
otherwise a later check takes it out before composing another. The log names
each reminded request and its reply ID, with whether the reminder reached the
agent, was taken back out of the queue, waits in the queue, or may have reached
it. A check that cannot read the bridge state, the reminder record, the pane, or
the session record, or cannot hand a reminder to the queue or take one back,
logs why once, and logs again once that step works.

When the service starts with reminders on and `reply-reminders.json` does not
exist, it creates the file and records the time. No request whose prompt was
delivered before that time is ever reminded, so the requests already unanswered
when reminders are first deployed get none. A release without reminders leaves
the file alone, so it survives a rollback. Deleting it is safe: the next check
creates a new one, and only requests delivered after that can be reminded. If it
cannot be read or is outside its bounds, no reminder is sent until it is
repaired or deleted, and the log names the file.

## Service management

`chat run` is a foreground process and exits cleanly after SIGINT or SIGTERM.
A service manager should restart it on failure and use the same state directory.
Size task and memory limits for the selected provider implementation; those
costs are outside the provider-neutral host and can differ substantially between
plugins. Disable swap for a latency-sensitive bridge only after giving the
provider enough physical-memory headroom.

`chat run` logs to standard error, and every line it writes begins with the UTC
time, to the second, at which it was written, so a service manager that appends
standard error to a plain file still records when each event happened. That
includes its final error line and every line of `graceful-stop-main`. Two kinds
of output have no time: an error in the command-line arguments, which is printed
before the command is known, and a panic message. Other commands, `chat tick`
among them, print their lines without a time. Each log line is written in one
piece, and a line that cannot be written, for example because the disk is full,
is dropped; the service keeps running. The provider worker logs one line each
time a provider subscription opens, such as
`2026-09-30T01:00:36Z agentctl: chat provider: subscribed from the saved cursor`,
and one line each time a subscription ends or fails, naming the reason and the
wait before the next attempt, such as
`agentctl: chat provider: stream ended; reconnecting in 1s` after the time. An
attempt runs from the start of connecting until its subscription ends or fails.
The provider reconnect wait starts at 1 second and doubles after each attempt
that lasted less than 60 seconds, up to 60 seconds. An attempt that lasted at
least 60 seconds counts as healthy, so the wait after it starts again at 1
second. A failed Herdr output subscription is logged the same way, with
`retrying in` and its wait. If the provider worker itself ends or panics while
the service is not stopping, `chat run` exits with an error, such as `chat
provider worker stopped unexpectedly`, so the service manager restarts it
instead of the bridge running on without chat events.

Derive the hard stop interval from the selected plugin manifest rather than
copying a universal number. Before Hello completes, the host may need
`hello_seconds + close_seconds + shutdown_grace_seconds + 7` seconds. After
Hello, it may need `close_seconds + shutdown_grace_seconds + 7`. The final seven
seconds reserve the process supervisor's two-second forced-reap bound and five
seconds for host reconciliation and worker joins. Start is interruptible after
Hello, so its independent phase timeout is not additive. An admitted reaction
or reply helper separately owns `outbound_command.timeout_millis +
outbound_command.shutdown_grace_millis + 7000` milliseconds. Set
`TimeoutStopSec` to at least the maximum of the applicable provider window and
that outbound window, plus measured service-manager scheduling margin. The
maximum accepted outbound window is 67 seconds. At startup the host pins the
complete selected provider timeout tuple. A later generation whose manifest
differs is rejected until the service restarts, so the configured outer bound
cannot silently become stale.

With systemd 258 or newer, a synchronous same-binary `ExecStop` can address only
the exact main-process identity. `graceful-stop-main` opens a pidfd for
systemd's `MAINPID`, requires its inode to equal `MAINPIDFDID`, sends SIGTERM
through that pidfd, and waits for exact process exit. It does not return at its
70-second diagnostic threshold, because returning would begin a second stop
phase. Pair it with `TimeoutStopFailureMode=kill`: at the one service-manager
deadline, systemd kills the complete control group rather than starting another
grace interval.

For example, a user service can run the same foreground command (replace every
absolute placeholder and tune the resource ceilings from a measured provider
probe):

```ini
[Unit]
Description=agentctl chat bridge
After=herdr.service

[Service]
Type=exec
Environment=AGENTCTL_HOME=/home/USER/.agentctl
ExecStart=/absolute/path/agentctl --registry /work/project/.agentctl chat run --bridge-state /home/USER/.local/state/agentctl/project-chat
ExecStop=/absolute/path/agentctl chat graceful-stop-main --main-pid ${MAINPID} --main-pidfd-id ${MAINPIDFDID}
Restart=on-failure
RestartSec=2
KillMode=control-group
OOMPolicy=kill
# Replace every value below with a measured deployment-specific value.
TimeoutStopSec=<DERIVED_SECONDS_WITH_MARGIN>
TimeoutStopFailureMode=kill
TasksMax=<MEASURED_TASKS_WITH_HEADROOM>
MemoryHigh=<MEASURED_RECLAIM_THRESHOLD>
MemoryMax=<MEASURED_HARD_LIMIT>
MemorySwapMax=0
CPUQuota=<MEASURED_CPU_LIMIT>

[Install]
WantedBy=default.target
```

After installing the unit as `agentctl-chat.service`, make persistence explicit
and verify both properties instead of merely starting an ephemeral process:

```sh
systemctl --user daemon-reload
systemctl --user enable --now agentctl-chat.service
systemctl --user is-enabled agentctl-chat.service
systemctl --user is-active agentctl-chat.service
```

`enable --now` survives a user-manager restart; operation while the user is
logged out additionally requires lingering to be enabled for that account.
`Type=exec` makes an `active` transition contingent on successful executable
startup. `KillMode=control-group` and the one derived stop deadline keep plugin
and helper descendants inside the unit's cleanup boundary. Use the same pinned
`agentctl` executable in `ExecStart` and `ExecStop`; a mutable symlink can make
the control helper disagree with the running generation.

The resource tokens in the example are intentionally invalid placeholders, not
defaults, minimums, or evidence about an unmeasured plugin. Measure the whole
service cgroup—including every plugin and helper descendant—under bounded
end-to-end load. Choose limits with explicit headroom, verify their parsed
systemd values after reload, and then prove the selected envelope still meets
the deployment's latency target. A `TasksMax` below a plugin's startup fan-out
will prevent that generation from launching; an excessively loose limit does
not provide useful containment.

The host bounds retained requests, replies, retirement records, commit receipts, per-frame data, event drains,
mailboxes, process cleanup admissions, and helper deadlines. Ordinary provider
or terminal events touch direct durable records. Full directory scans happen at
startup and explicit/periodic recovery, not in the subscription inner loop.

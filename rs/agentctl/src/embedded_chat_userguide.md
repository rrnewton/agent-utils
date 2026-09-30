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
file, is not posted, except in a read longer than the screen, as described
below. Its ID is not reported as unavailable either, unless the first row of
that item is out of view, the item is a Claude Code message whose first row is
the top row of the read, or the read is longer than the screen, as described
below. A prompt starts
at a row whose text after its indentation is `❯`, `›`, `↳` or `»`, alone or
followed by a space or a no-break space, and its wrapped rows continue two or
more columns further right. Claude Code draws a prompt and its input box after
`❯`; Codex draws a prompt after `›`, a prompt it holds until a running tool call
ends and a hook's notice after `↳`, and its input box after `»`. Tool output
starts at a row whose text after its indentation begins with `⎿`, or is `└`
alone or followed by a space, and its later rows continue right of that
character. Codex draws `└` alone when the output's first line is empty. A code
fence in tool output neither opens nor closes a fence.

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

The bridge asks herdr for the newest 4,000 lines of the pane. For a Claude Code
pane herdr usually returns the rows on the screen, and for a Codex pane at most
about 1,000 lines. The rows above the first nonblank row that starts at the left
edge continue an item whose first row is out of view, which can be a prompt or
tool output, so an opening marker there opens no block, although an unavailable
ID in it is still reported, and a code fence that opens there ends at that row.
Once a prompt has scrolled off the top of the screen, Claude Code can draw a
copy of it over the screen's top row, at the left edge and cut to one row, above
rows of whatever item the screen starts in. So a `❯` row at the left edge that
is the first nonblank row of a capture is skipped: it starts no prompt, and it
is not the first row at the left edge. A Claude Code prompt that really starts
at the top of a capture is then read as rows above the first row at the left
edge, so a block it quotes is not posted, but it is reported unless its text is
the end of a stored reply. Codex draws no such copy, so a Codex prompt row at
the top of a capture starts a prompt, as it does anywhere else. A block is
therefore posted only by a capture that shows both
the first row of the message that holds it and the whole block. A block that
starts its message needs only the whole block in view; a block after other text
in its message also needs the message's first row, which leaves the screen
sooner. A block taller than the screen is usually not posted from a Claude Code
pane. A capture that sees only part of a block handles that part as a partial
block, as described below, except in the longer reads described next.

A Claude Code pane keeps no scrollback, yet herdr can return more lines than
its screen has rows. Herdr 0.8.0 builds such a read of an agent that is idle and
takes mouse wheel input: it scrolls the agent's view up with wheel events, joins
the screens it sees where they overlap, and scrolls the view back down. That
can take up to 20 seconds, and anyone watching the pane sees the view move. The
join keeps each row that differs from the row at the same position on the
screen before, so each time the copy of a prompt that Claude Code draws over
the top row changes, the copy lands between rows of the conversation. The extra
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
inside is reported as having no closing marker, although
the agent wrote one. A later read that shows only
the screen reads the block by the rules above, if it is still in view: it is
posted if that read also shows the first row of its message, and otherwise
reported, unless a block under its ID was already reported.

Replies are recognized by their text, not by the number in their reply ID. A
reply ID is the request's nonce, an underscore, and a number from 1 to 999999
with no leading zero. When a block appears under any reply ID of an open
request, the bridge compares its text with the replies it has already stored for
that request. The comparison ignores whitespace and the box-drawing characters,
U+2500 to U+257F, that a terminal draws for table borders, so a paragraph the
terminal re-wraps or a border it redraws at another width still matches. Every
other character counts, including the block elements of a progress bar. A block
whose text matches a stored reply is not posted again; any other block is stored
as the request's next reply and posted. So an answer the agent sends twice to
one request, under one ID or under two, is posted once, and two identical short
replies to one request, such as two `ok` progress notes, are posted as one. The
same text sent to two requests is posted to each.

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
and provider faults are errors.

A reply marker in the pane whose ID is not available, such as a typo or a stale
block left in scrollback by another bridge state, produces one routing-error
prompt to the agent. An ID is unavailable when its number is malformed or it
names no request the bridge knows. A well-formed ID of a closed request is
ignored. The prompt names the unavailable ID and up to 32 of the reply IDs that
were available when it was written, and counts the rest. It is never posted to
chat. Reported IDs are kept in `fence-feedback.json` in the bridge state
directory, so each unavailable ID is reported at most once per state directory,
including after a restart. A new state directory starts with no reported IDs. A
marker that stays visible after its report is left out of later prompts, and a
later block that reuses a reported ID is not reported again. A recovery scan
also reports the ID of an open request's block that it sees only in part, such
as an opening marker before its closing one, or a closing marker whose opening
one has scrolled away, unless the visible part holds no text or is the start or
the end of a reply already stored for that request, read either way. A part cut
inside a table row that wrapped at another width than the stored reply's is
still reported, once. The prompt then names that ID as unavailable, and also as
available if it is among the available IDs it lists. Since reports are kept by
ID, the first partial block reported under an ID uses up that ID's report, even
when the agent did not write that block or did finish it: a block another agent
quoted in a message whose first row is out of view, including a Claude Code
message under the copy of its first row or one whose first row is the top row of
the read; a block a tool printed, in output whose first row is out of view; or,
in a read longer than the screen, a quoted block, or the agent's own block with
a copied prompt row or a tool-output row inside it. A later partial
block under that ID, including the agent's own, is then only logged. A complete
block under that
ID is still posted if its text is new. A block under an ID that belongs to no
open request is never posted. Once its ID is reported, its only trace is a log
line, and only recovery scans write that line. Recovery scans run when
`chat run` starts, at each `chat tick`, at each reconciliation while the agent
pane is idle or done, when the pane settles idle
or done, and after output names a reply ID that belongs to no request the bridge
knows. Each recovery scan that sees reported IDs that are still unavailable logs
one `already reported, so not repeated` line that names up to 8 of them and
counts the rest, up to 128 per scan. Other captures do not write that line, so a
reused ID that leaves the screen before the next recovery scan leaves no trace.
Already reported markers are set aside before the bound on new ones, so a screen
full of old markers cannot hide a new one. While a routing-error prompt is still
queued, newer unavailable IDs wait for it instead of producing a second prompt.
The exact pending prompt is saved before submission, so recovery settles its
original queue ID even if a crash hides the submission result or newer
unavailable markers appear. A prompt whose queue outcome is uncertain counts as
reported: it is not submitted again, even if it never reached the agent. The
history retains up to 4,096 distinct reported or pending IDs. At that limit, new
diagnostics stay held; reported IDs are never evicted or submitted again.
Deleting `fence-feedback.json` clears the history, so the IDs it held can be
reported once more. If the file cannot be read or is outside its bounds, the
error names it and diagnostics stay held until it is repaired or deleted.

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

# Slack provider for the agentctl chat bridge

This crate holds two executables for an `agentctl chat` bridge:

* `agentctl-slack-plugin`, the chat-subscription plugin `slack`
  (capability `chat-subscription.slack`). It speaks
  `agentctl-chat-subscription` v1 on stdin/stdout and turns `message.created`
  events from one Slack channel into ordered, durably cursored requests.
* `agentctl-slack-outbound`, the bridge's one-shot `outbound_command`. It posts
  replies into the request's Slack thread and adds the acknowledgement
  reactions, verifies human authors before rejection reactions, and supports
  read-only sender lookup and channel member listings
  for temporary chat grants. It always uses a registered Slack app
  (`--app-id=A…`). Without an app id it refuses every request and contacts
  nothing.

Both reach Slack through the `SlackTransport` interface. The implemented
transport runs an installed Slack command-line client whose absolute path the
operator configures; a Slack Web API transport is a placeholder that refuses.
`fake::FakeTransport` is the deterministic transport the tests use.

`manifest.json` is the plugin manifest to install beside the plugin binary in
`$AGENTCTL_HOME/plugins/slack/`. The configuration, the cursor and recovery
model, and a recipe for running a separate bridge for one test agent are in
`agentctl chat userguide`, section "Slack provider".

### Sender lookup and channel membership

`agentctl chat grant` can resolve a unixname or email to the Slack user ID the
bridge stores. The helper runs `slack.user describe --user=QUERY
--columns=user_id --output=json`. Raw Slack IDs (`U…` or `W…`) also work; find
one in the person's Slack profile if directory lookup is unavailable.

For `grant --all-members`, the helper uses `slack.conversation members
--channel-id=C… --limit=20 --columns=user_id --output=json` and follows every
`--page-token`. It returns sorted, distinct IDs, never a wildcard. A person
joining later is not included. Every page must confirm complete source data,
zero dropped rows, and consistent pagination; the helper refuses incomplete
results, repeated page tokens, or more than 256 distinct members. All pages
share the helper's `--timeout-ms` deadline (25,000 ms by default). If a complete
listing is unavailable, supply the intended people explicitly.

These operations extend the helper's existing version 1 request protocol:

```json
{"version":1,"id":"<uuid>","action":"resolve_sender","channel_id":"C…","query":"<unixname|email|ID>"}
{"version":1,"id":"<uuid>","action":"list_members","channel_id":"C…"}
```

The matching success receipts are `{"sender_id":"U…"}` and
`{"sender_ids":["U…","W…"]}`. Both actions require the channel in the helper's
`--channel` list, follow the configured client identity and environment, and
perform no Slack writes or operation-ledger writes. Failures use the existing
bound `id`/`action` error envelope with outcome `not_applied`.

### Rejection reactions

Eligible new messages from unallowed senders and messages with an ignored prefix carry only
channel, message, sender and rejection reason to the bridge. They do not become requests or
create reply routes. Edits, deletes, bot-message subtypes, channel notices, invalid identifiers,
other channels and messages before the lineage start remain checkpoints without rejection
feedback. Message `app_id` and `bot_id` annotations do not establish whether the author is human.

The helper's guarded action is:

```json
{"version":1,"id":"<uuid>","action":"ensure_rejection","channel_id":"C…","message_id":"C…/messages/<ts>","sender_id":"U…","emoji":"❌"}
```

Before each actual write it runs `slack.user describe --user=ID --columns=user_id,is_bot --output=json` and requires
that exact ID with boolean `is_bot=false`. Bot accounts, including the bridge's own U-prefixed
bot user, and unknown or ambiguous account data cause no reaction. The helper returns
`author_not_human`, `not_applied`, nonretryable for a fresh operation. Lookup errors also stop
the write. A confirmed human gets the configured reaction as the app; ❌ maps to Slack's `x`.
The lookup, durable intent and write share one `--timeout-ms` deadline, follow the configured
client identity, and use the standard reaction receipt. Every actual attempt rechecks the
account; a completed operation returns its saved receipt without another lookup or write.

The existing mandatory `--ledger` also holds `rejections/<UUID>.json` and an empty
`rejections/<UUID>.lock`. Before a possible write the helper atomically saves and syncs the
UUID's app, channel, message, sender and first normalized reaction name. A retry uses that
original name even after the configured emoji changes or the bridge evicts a completed outbox
row. Reusing the UUID for another app, channel, message or sender is refused. Completed replay
returns the original reaction ID with `already_present=true`. If an earlier attempt is still
uncertain, a later failed account check performs no write and preserves the operation's
`unknown` outcome. Fresh bot or lookup refusals create no reaction intent; they may create the
private lock file. Directory reads still create neither locks nor intents.

Retain the rejection journal with the bridge state, including completed records and lock files.
The helper never evicts them: removing them loses the original-choice binding and can allow a
later replay to add a different emoji. Each record is at most 4 KiB; storage grows by one small
record per possible reaction write and one lock per checked operation. The configured ledger's
parent directory must already exist; it is the provisioned storage anchor. The helper creates only the ledger leaf and its
rejection journal beneath it, and syncs their parent entries before a possible effect.
Journal paths cannot contain symlinks or parent traversal. The ledger and rejection directories
and single-linked regular files must be private and owned by the helper's user. A busy
operation lock returns immediately as `rejection_ledger_busy`; unsafe or
unreadable storage and failed durable writes return `rejection_ledger_unreadable` or
`rejection_ledger_unwritable`, with an unknown outcome and no new Slack write after a failed
pre-write check. Identity conflicts return nonretryable `rejection_conflict`.

Normal acknowledgement and delivery reactions are unchanged. Rejection feedback is best
effort and does not hold back the inbound cursor.

Build, test, and stage the files the guide's recipe installs (`BUILD` there):

```sh
cargo test --manifest-path rs/Cargo.toml -p chat-subscription-slack
cargo build --release --manifest-path rs/Cargo.toml -p chat-subscription-slack
BUILD="$HOME/.local/state/agentctl-slack-test/build"
install -m 0700 -d "$BUILD"
install -m 0500 rs/target/release/agentctl-slack-plugin \
  rs/target/release/agentctl-slack-outbound "$BUILD/"
install -m 0400 rs/chat-subscription-slack/manifest.json "$BUILD/"
```

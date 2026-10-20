# Slack provider for the agentctl chat bridge

This crate holds two executables for an `agentctl chat` bridge:

* `agentctl-slack-plugin`, the chat-subscription plugin `slack`
  (capability `chat-subscription.slack`). It speaks
  `agentctl-chat-subscription` v1 on stdin/stdout and turns `message.created`
  events from one Slack channel into ordered, durably cursored requests.
* `agentctl-slack-outbound`, the bridge's one-shot `outbound_command`. It posts
  replies into the request's Slack thread and adds the acknowledgement
  reactions, and supports read-only sender lookup and channel member listings
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

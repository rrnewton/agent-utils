# Slack provider for the agentctl chat bridge

This crate holds two executables for an `agentctl chat` bridge:

* `agentctl-slack-plugin`, the chat-subscription plugin `slack`
  (capability `slack.events.cli`). It speaks
  `agentctl-chat-subscription` v1 on stdin/stdout and turns `message.created`
  events from one Slack channel into ordered, durably cursored requests.
* `agentctl-slack-outbound`, the bridge's one-shot `outbound_command`. It posts
  replies into the request's Slack thread and adds the acknowledgement
  reactions, always as a registered Slack app (`--app-id=A…`). Without an app
  id it refuses every request and contacts nothing.

Both reach Slack through the `SlackTransport` interface. The implemented
transport runs an installed Slack command-line client whose absolute path the
operator configures; a Slack Web API transport is a placeholder that refuses.
`fake::FakeTransport` is the deterministic transport the tests use.

`manifest.json` is the plugin manifest to install beside the plugin binary in
`$AGENTCTL_HOME/plugins/slack/`. The configuration, the cursor and recovery
model, and a recipe for running a separate bridge for one test agent are in
`agentctl chat userguide`, section "Slack provider".

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

//! `agentctl-slack-plugin`: the Slack chat-subscription process plugin.
//!
//! The bridge starts it from its plugin manifest and speaks `agentctl-chat-subscription` v1 on
//! standard input and output. All settings arrive in the protocol's Start frame.

use chat_subscription_slack::plugin::SlackBackend;

fn main() {
    let mut backend = SlackBackend::with_default_transport();
    if let Err(error) = chat_subscription_plugin::serve_stdio(&mut backend) {
        eprintln!("agentctl-slack-plugin: {error}");
        std::process::exit(1);
    }
}

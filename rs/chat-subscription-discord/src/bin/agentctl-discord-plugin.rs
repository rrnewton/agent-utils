//! `agentctl-discord-plugin`: the Discord chat-subscription process plugin.
//!
//! The bridge starts it from its plugin manifest and speaks `agentctl-chat-subscription` v1 on
//! standard input and output. All settings arrive in the protocol's Start frame
//! (`backend_configuration`, schema `agentctl.discord.v1`). Diagnostics go to standard error
//! and never include the bot token.

use chat_subscription_discord::plugin::DiscordBackend;

fn main() {
    let mut backend = DiscordBackend::with_default_transport();
    if let Err(error) = chat_subscription_plugin::serve_stdio(&mut backend) {
        eprintln!("agentctl-discord-plugin: {error}");
        std::process::exit(1);
    }
}

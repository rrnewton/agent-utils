//! `agentctl-slack-outbound`: the bridge's one-shot outbound helper for Slack.
//!
//! Reads one JSON request line from standard input, posts the reply or reaction as the
//! configured Slack app, and writes one JSON response line to standard output. It always exits
//! 0 after writing a response; the response says whether the operation succeeded.

use std::io::{Read, Write};

use chat_subscription_slack::diag::unix_millis;
use chat_subscription_slack::outbound::{handle_request, OutboundConfig, MAX_REQUEST_BYTES};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let config = OutboundConfig::parse(&arguments);
    let mut input = Vec::new();
    let limit = u64::try_from(MAX_REQUEST_BYTES + 1).unwrap_or(u64::MAX);
    if let Err(error) = std::io::stdin().take(limit).read_to_end(&mut input) {
        eprintln!("agentctl-slack-outbound: reading the request failed: {error}");
        std::process::exit(1);
    }
    let mut build = || match &config {
        Ok(config) => config.transport(),
        Err(problem) => Err(problem.clone()),
    };
    let response = handle_request(
        &input,
        config.as_ref().map_err(String::as_str),
        &mut build,
        &unix_millis,
    );
    let mut stdout = std::io::stdout().lock();
    if stdout
        .write_all(&response)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        std::process::exit(1);
    }
}

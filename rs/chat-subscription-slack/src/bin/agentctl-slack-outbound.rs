//! `agentctl-slack-outbound`: the bridge's one-shot outbound helper for Slack.
//!
//! Reads one JSON request line from standard input, posts the reply or reaction as the
//! configured Slack app, and writes one JSON response line to standard output. It always exits
//! 0 after writing a response; the response says whether the operation succeeded.
//!
//! With `--role=warm-worker` it is instead the long-lived warm worker that holds spare Slack
//! clients for the helper's posts (see the library's `warm` module); it exits 1 only when it
//! cannot serve its socket.

use std::io::{Read, Write};

use chat_subscription_slack::diag::unix_millis;
use chat_subscription_slack::outbound::{handle_request, OutboundConfig, Role, MAX_REQUEST_BYTES};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let config = OutboundConfig::parse_with_environment(
        &arguments,
        std::env::var("AGENTCTL_SLACK_FORMAT").ok().as_deref(),
    );
    if config.is_err()
        && arguments
            .iter()
            .any(|argument| argument == "--role=warm-worker")
    {
        // A worker has no request to answer, so its argument error goes to standard error.
        eprintln!(
            "agentctl-slack-outbound: warm worker: {}",
            config.as_ref().err().map_or("", String::as_str)
        );
        std::process::exit(2);
    }
    if let Ok(config) = &config {
        if config.role == Role::WarmWorker {
            // The warm worker runs until it fails; it reads no request from standard input.
            if let Err(problem) = config.serve_warm() {
                eprintln!("agentctl-slack-outbound: warm worker: {problem}");
            }
            std::process::exit(1);
        }
    }
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

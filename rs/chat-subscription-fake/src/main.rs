//! Standalone conformance plugin used by process-protocol tests and host smoke checks.

#![forbid(unsafe_code)]

use chat_subscription_fake::FakeBackend;

fn main() {
    let mut backend = FakeBackend::conformance("fake-process-plugin");
    if let Err(error) = chat_subscription_plugin::serve_stdio(&mut backend) {
        eprintln!("fake chat plugin failed: {error}");
        std::process::exit(1);
    }
}

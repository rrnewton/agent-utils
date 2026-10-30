//! Replays the fixtures under `tests/fixtures/`: each harness's transcript yields the replies as
//! written, and the screen's rendering of a reply is recognised as that reply.

use std::path::PathBuf;

use agent_harness::replies::{complete_blocks, rendered_equivalent, ReplyBlock};
use agent_harness::{decode_all, transcript, Event, Follower};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Every complete reply block in the model's text of the fixture `name`, read as `harness`.
fn replies(harness: &str, name: &str) -> Vec<ReplyBlock> {
    let bytes = std::fs::read(fixture(name)).expect("fixture");
    decode_all(transcript(harness).expect("harness"), &bytes)
        .into_iter()
        .filter_map(|event| match event {
            Event::Text { text, .. } => Some(text),
            _ => None,
        })
        .flat_map(|text| complete_blocks(&text))
        .collect()
}

const WRITTEN: &str = "**Mitigated; please reload.** The cause was `rate limiting`, see the [status page](https://example.com/s).\n\n| Leg | Before | After |\n|---|---:|---:|\n| admitted to delivered | 990 s | 5.6 s |";

#[test]
fn a_claude_session_yields_the_mid_turn_and_final_replies_and_no_quoted_ones() {
    let found = replies("claude", "claude-session.jsonl");
    let bodies: Vec<&str> = found.iter().map(|block| block.body.as_str()).collect();
    assert_eq!(
        bodies,
        [
            "Progress: the gate is running on the rebased branch; I'll report when it finishes.",
            WRITTEN,
        ]
    );
    assert!(found.iter().all(|block| block.identifier == "041"));
}

#[test]
fn muse_and_codex_transcripts_yield_the_reply_as_written() {
    for (harness, name) in [
        ("muse", "muse-session.jsonl"),
        ("codex", "codex-rollout.jsonl"),
    ] {
        let found = replies(harness, name);
        assert_eq!(found.len(), 1, "{harness}");
        assert_eq!(found[0].body, WRITTEN, "{harness}");
    }
}

#[test]
fn the_screens_rendering_is_recognised_and_the_other_reply_is_not() {
    let screen = std::fs::read_to_string(fixture("screen-reply.txt")).expect("screen");
    let found = replies("claude", "claude-session.jsonl");
    let matching: Vec<&str> = found
        .iter()
        .filter(|block| rendered_equivalent(&screen, &block.body))
        .map(|block| block.body.as_str())
        .collect();
    assert_eq!(matching, [WRITTEN]);
}

#[test]
fn a_follower_reads_the_fixture_once() {
    let reader = transcript("claude").expect("claude");
    let mut follower = Follower::open(&fixture("claude-session.jsonl"), reader.decoder(), 1 << 20);
    let events = follower.poll().expect("readable");
    assert!(matches!(events.first(), Some(Event::Prompt { .. })));
    assert!(matches!(events.last(), Some(Event::TurnEnd { .. })));
    assert!(follower.poll().expect("again").is_empty());
}

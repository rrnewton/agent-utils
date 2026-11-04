//! Replay tests for #714 chat-transcript-primary (main-session trace).

use std::time::Duration;

use agentctl::chat_trace::{Collector, TracePost};

fn replay(reader: &str, bytes: &str) -> Vec<TracePost> {
    let reader = agent_harness::transcript(reader).unwrap();
    let events = agent_harness::decode_all(reader, bytes.as_bytes());
    let mut collector = Collector::new(Duration::from_secs(45));
    let mut posts = Vec::new();
    for event in &events {
        posts.extend(collector.feed(event));
    }
    posts
}

fn user(at: &str, text: &str) -> String {
    serde_json::json!({"type":"user","timestamp":at,"message":{"role":"user","content":text}})
        .to_string()
}
fn asst(at: &str, text: &str) -> String {
    serde_json::json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"text","text":text}]}}).to_string()
}
fn end(at: &str) -> String {
    serde_json::json!({"type":"system","subtype":"turn_duration","timestamp":at}).to_string()
}

#[test]
fn claude_replay_traces_only_unfenced_text() {
    let bytes = [
        user(
            "2026-11-02T12:00:00Z",
            "[from Ryan via slack #chan · thread T7 · msg m1]\nfix it",
        ),
        asst(
            "2026-11-02T12:00:01Z",
            "<CHAT_REPLY_001>\nposted answer\n</CHAT_REPLY_001>",
        ),
        asst(
            "2026-11-02T12:00:02Z",
            "Internal note with findings the chat never saw.",
        ),
        end("2026-11-02T12:00:03Z"),
    ]
    .join("\n");
    let posts = replay("claude", &bytes);
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert!(posts[0].text.contains("Internal note"));
    assert!(!posts[0].text.contains("posted answer"));
}

#[test]
fn muse_replay_traces_assistant_text() {
    let intent = |micros: u64, text: &str| {
        serde_json::json!({"recorded_at":micros,"payload_type":"runtime.user_intent.accepted","payload":{"model_messages":[{"content":[{"kind":"text","text":text}]}]}}).to_string()
    };
    let run = |kind: &str, extra: &str, micros: u64| {
        format!(
            r#"{{"recorded_at":{micros},"payload_type":"runtime.session","payload":{{"kind":"run","event":{{"kind":"{kind}"{extra}}}}}}}"#
        )
    };
    let bytes = [
        intent(1_000_000, "[from lane-a via agentctl] work"),
        run(
            "assistant_message_committed",
            r#","text":"Trace-worthy working notes.""#,
            2_000_000,
        ),
        run("terminal", "", 3_000_000),
    ]
    .join("\n");
    let posts = replay("muse", &bytes);
    assert_eq!(posts.len(), 1, "{posts:?}");
    // #832: the trigger is metadata only; the text is the agent's words.
    assert_eq!(posts[0].trigger, "agent lane-a");
    assert_eq!(posts[0].text, "Trace-worthy working notes.");
}

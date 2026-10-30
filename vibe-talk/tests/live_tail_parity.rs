//! `#709 vt-live-tail-rust`: the in-server tail and the retired Python adapter agree.
//!
//! One recorded stream (fixture lines below, in the shape
//! the provider CLI's `conversation tail --output=json --include-iris-seq-id` writes, identifiers and
//! text replaced) is replayed twice against identical fresh servers on the loopback Slack
//! fake:
//!
//! * **Adapter path.** Each line goes through the retired adapter's forwarding decisions,
//!   re-implemented here line for line from `live-hint-adapter.py`'s records mode as it last
//!   shipped (kept in git history): heartbeats dropped locally, an unreadable or oversize line
//!   posted as a gap, every other record POSTed verbatim to `/api/v1/live/tail`, the saved
//!   position advanced after each acknowledged record — a 400 refusal included. Before the
//!   adapter was deleted, this harness's expectations were checked against the real Python
//!   adapter's `--self-test` and a scripted replay (see the issue's handoff).
//! * **Supervisor path.** Each line goes through
//!   [`vibe_talk::live_tail_supervisor::apply_line`], the in-server supervisor's per-record
//!   step, with its own cursor file.
//!
//! The two runs must end with the same per-line outcomes, the same messages published to the
//! live hub, and the same saved position. Any drift between what the HTTP route does with a
//! record and what the supervisor does with it fails here, not in production.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::live_tail::TailOutcome;
use vibe_talk::live_tail_supervisor::{apply_line, ApplyOutcome, TailCursor};
use vibe_talk::model::ChannelId;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{READ_TOKEN, WRITE_TOKEN};

const CH: &str = "C0000MAIN1";
const UNLISTED: &str = "C0000OTHR2";
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

fn now_ts(offset: i64, micros: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    format!(
        "{}.{micros:06}",
        i64::try_from(now).expect("a sane clock") + offset
    )
}

fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(CH.to_owned(), Vec::new());
    state.users.insert(
        "UGRACE001".to_owned(),
        json!({"id": "UGRACE001", "name": "grace", "profile": {"display_name": "Grace"}}),
    );
    let _ = profiled; // The fake's history starts empty; every message arrives by stream.
    state
}

fn deployment(fake: &Fake) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[ingest]
token = "{INGEST_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
live_poll_seconds = 3600

[[channels]]
id = "{CH}"
label = "team"
writable = true
"#,
        base = fake.base,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

fn record(seq: u64, conversation: &str, ts: &str, user: &str, text: &str) -> Vec<u8> {
    json!({
        "type": "message.created",
        "iris_seq_id": seq,
        "payload": {
            "container_id": conversation,
            "slack_message": {
                "channel_id": conversation,
                "message_ts": ts,
                "thread_ts": null,
                "slack_user_id": user,
                "bot_id": null,
                "app_id": null,
                "subtype": null,
                "text": text,
                "team_id": "T0000TEAM",
            },
        },
    })
    .to_string()
    .into_bytes()
}

/// The recorded stream: every forwarding decision the adapter makes, in order.
fn stream() -> Vec<Vec<u8>> {
    let first = now_ts(-30, 100);
    let second = now_ts(-20, 200);
    let mut lines: Vec<Vec<u8>> = vec![
        b"\n".to_vec(),
        json!({"type": "HEARTBEAT"}).to_string().into_bytes(),
        record(5, CH, &first, "UGRACE001", "first words"),
        // The same record again (a replay after an uncertain delivery): known, not republished.
        record(5, CH, &first, "UGRACE001", "first words"),
        record(6, CH, &second, "UGRACE001", "second words"),
        json!({"type": "GAP", "lost_from_iris_seq_id": 3, "lost_to_iris_seq_id": 4})
            .to_string()
            .into_bytes(),
        json!({"type": "reaction.created", "iris_seq_id": 7, "payload": {
            "container_id": CH, "slack_reaction": {"channel_id": CH}}})
        .to_string()
        .into_bytes(),
        b"not json".to_vec(),
        // A record the checker refuses (bad user id): skipped, position moves past it.
        record(8, CH, &now_ts(-10, 300), "ada; rm -rf", "refused words"),
        // A conversation this server does not list: acknowledged and dropped.
        record(9, UNLISTED, &now_ts(-5, 400), "UGRACE001", "elsewhere"),
        json!({"type": "STALL"}).to_string().into_bytes(),
        json!({"type": "REVIVAL"}).to_string().into_bytes(),
        // Sequence shapes neither implementation may read as a position: a boolean and a
        // float. Both refuse the record as malformed; the saved position must not move.
        json!({"type": "message.created", "iris_seq_id": true, "payload": {
            "container_id": CH, "slack_message": {"channel_id": CH,
            "message_ts": now_ts(-4, 500), "slack_user_id": "UGRACE001",
            "text": "boolean sequence", "team_id": "T0000TEAM"}}})
        .to_string()
        .into_bytes(),
        json!({"type": "message.created", "iris_seq_id": 9.5, "payload": {
            "container_id": CH, "slack_message": {"channel_id": CH,
            "message_ts": now_ts(-3, 600), "slack_user_id": "UGRACE001",
            "text": "float sequence", "team_id": "T0000TEAM"}}})
        .to_string()
        .into_bytes(),
    ];
    // One line no record bound can carry.
    let mut oversize = b"{".to_vec();
    oversize.resize(vibe_talk::slack::tail::MAX_RECORD_BYTES + 10, b'x');
    oversize.push(b'}');
    lines.push(oversize);
    lines
}

async fn post(state: &AppState, body: Vec<u8>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/live/tail")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {INGEST_TOKEN}"))
        .body(Body::from(body))
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn published_texts(state: &AppState) -> Vec<(String, String)> {
    state
        .live
        .subscribe(&channel(), None)
        .replay
        .into_iter()
        .map(|live| (live.message.id.to_string(), live.message.content.clone()))
        .collect()
}

const GAP_SKIPPED: &[u8] =
    br#"{"type":"GAP","reason":"the adapter skipped a record it could not read"}"#;

/// The adapter's records loop, as it last shipped, with its cursor held in memory.
async fn adapter_path(state: &AppState, lines: &[Vec<u8>]) -> (Vec<String>, Option<u64>) {
    let mut outcomes = Vec::new();
    let mut cursor: Option<u64> = None;
    let mut advance = |sequence: Option<u64>| {
        if let Some(sequence) = sequence {
            if cursor.is_none_or(|held| sequence > held) {
                cursor = Some(sequence);
            }
        }
    };
    for line in lines {
        let candidate: &[u8] = {
            let mut line = line.as_slice();
            if line.last() == Some(&b'\n') {
                line = &line[..line.len() - 1];
            }
            let start = line
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .unwrap_or(line.len());
            let end = line
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map_or(start, |i| i + 1);
            &line[start..end]
        };
        if candidate.is_empty() {
            outcomes.push("local-skip".to_owned());
            continue;
        }
        if candidate.len() > vibe_talk::slack::tail::MAX_RECORD_BYTES {
            let (_, body) = post(state, GAP_SKIPPED.to_vec()).await;
            outcomes.push(body["outcome"].as_str().unwrap_or("?").to_owned());
            continue;
        }
        let value: Option<Value> = serde_json::from_slice(candidate)
            .ok()
            .filter(Value::is_object);
        let Some(value) = value else {
            let (_, body) = post(state, GAP_SKIPPED.to_vec()).await;
            outcomes.push(body["outcome"].as_str().unwrap_or("?").to_owned());
            continue;
        };
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if ["HEARTBEAT", "STALL", "REVIVAL"].contains(&kind) {
            outcomes.push("local-skip".to_owned());
            continue;
        }
        let sequence = value.get("iris_seq_id").and_then(Value::as_u64);
        let (status, body) = post(state, candidate.to_vec()).await;
        if status == StatusCode::BAD_REQUEST {
            outcomes.push("refused".to_owned());
        } else {
            outcomes.push(body["outcome"].as_str().unwrap_or("?").to_owned());
        }
        advance(sequence);
    }
    (outcomes, cursor)
}

fn outcome_name(outcome: &ApplyOutcome) -> String {
    match outcome {
        ApplyOutcome::Blank | ApplyOutcome::Control => "local-skip".to_owned(),
        ApplyOutcome::Gapped => "gap".to_owned(),
        ApplyOutcome::Refused => "refused".to_owned(),
        ApplyOutcome::Failed => "failed".to_owned(),
        ApplyOutcome::Applied(outcome) => match outcome {
            TailOutcome::Published => "published",
            TailOutcome::Known => "known",
            TailOutcome::History => "history",
            TailOutcome::Ignored => "ignored",
            TailOutcome::Unlisted => "unlisted",
            TailOutcome::Gap(_) => "gap",
            TailOutcome::Hinted(_) => "hinted",
        }
        .to_owned(),
    }
}

/// A deployment whose server supervises a fake tail: a Python one-liner standing in for the
/// provider CLI, printing one record and then sleeping, and noting the catchup it was given.
fn deployment_tail(fake: &Fake, directory: &std::path::Path) -> AppState {
    let program = "import json,sys,time; \
        open(sys.argv[3],'w').write(sys.argv[2].split('=',1)[1]); \
        ts='%.6f'%time.time(); \
        print(json.dumps({'type':'HEARTBEAT'}), flush=True); \
        print(json.dumps({'type':'message.created','iris_seq_id':42,'payload':{'container_id':sys.argv[4],'slack_message':{'channel_id':sys.argv[4],'message_ts':ts,'slack_user_id':'UGRACE001','text':'from the tail','team_id':'T0000TEAM'}}}), flush=True); \
        time.sleep(30)";
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[ingest]
token = "{INGEST_TOKEN}"
tail_command = ["python3", "-c", "{program}", "--channel-id={{channels}}", "--catchup={{catchup}}", "{marker}", "{CH}"]
tail_channels = ["{CH}"]
tail_state_file = "{state_file}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
live_poll_seconds = 3600

[[channels]]
id = "{CH}"
label = "team"
writable = true
"#,
        base = fake.base,
        marker = directory.join("catchup.txt").display(),
        state_file = directory.join("live-tail.json").display(),
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

#[tokio::test]
async fn the_supervised_tail_publishes_and_resumes_from_its_saved_position() {
    let fake = Fake::start(fixture(), 20).await;
    let directory =
        std::env::temp_dir().join(format!("live-tail-supervised-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("directory");
    let state = deployment_tail(&fake, &directory);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(vibe_talk::live_tail_supervisor::run(
        state.clone(),
        shutdown_rx,
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let saved: Option<u64> = std::fs::read(directory.join("live-tail.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|value| value["cursor"].as_u64());
        if published_texts(&state).len() == 1 && saved == Some(42) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the tail did not publish and save"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        std::fs::read_to_string(directory.join("catchup.txt")).expect("catchup noted"),
        "none",
        "a fresh tail starts where tail_initial_catchup says"
    );
    shutdown_tx.send(true).expect("shutdown sends");
    tokio::time::timeout(std::time::Duration::from_secs(15), running)
        .await
        .expect("the supervisor stops")
        .expect("the supervisor task holds");

    // A second run resumes from the saved position instead of starting over.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(vibe_talk::live_tail_supervisor::run(
        state.clone(),
        shutdown_rx,
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if std::fs::read_to_string(directory.join("catchup.txt")).is_ok_and(|held| held == "42") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the tail did not resume"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    shutdown_tx.send(true).expect("shutdown sends");
    tokio::time::timeout(std::time::Duration::from_secs(15), running)
        .await
        .expect("the supervisor stops")
        .expect("the supervisor task holds");
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn the_rust_tail_and_the_adapter_publish_the_same_records() {
    let lines = stream();

    let fake_a = Fake::start(fixture(), 20).await;
    let state_a = deployment(&fake_a);
    let (adapter_outcomes, adapter_cursor) = adapter_path(&state_a, &lines).await;
    let adapter_published = published_texts(&state_a);

    let fake_b = Fake::start(fixture(), 20).await;
    let state_b = deployment(&fake_b);
    let directory = std::env::temp_dir().join(format!("live-tail-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let mut cursor = TailCursor::new(directory.join("live-tail.json"));
    cursor.load().expect("cursor loads");
    let now = jiff::Timestamp::now().as_millisecond();
    let mut rust_outcomes = Vec::new();
    for line in &lines {
        rust_outcomes.push(outcome_name(
            &apply_line(&state_b, &mut cursor, line, now).await,
        ));
    }
    let rust_published = published_texts(&state_b);
    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(rust_outcomes, adapter_outcomes, "per-line outcomes differ");
    assert_eq!(
        rust_published, adapter_published,
        "published messages differ"
    );
    assert_eq!(cursor.value(), adapter_cursor, "saved positions differ");
    // The fixture really exercised the decisions it names, not a vacuous pass.
    assert!(adapter_outcomes.iter().any(|o| o == "published"));
    assert!(adapter_outcomes.iter().any(|o| o == "known"));
    assert!(adapter_outcomes.iter().any(|o| o == "gap"));
    assert!(adapter_outcomes.iter().any(|o| o == "refused"));
    assert!(adapter_outcomes.iter().any(|o| o == "unlisted"));
    assert_eq!(adapter_published.len(), 2);
    assert_eq!(adapter_cursor, Some(9));
}

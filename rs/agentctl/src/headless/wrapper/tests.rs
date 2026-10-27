//! The wrapper against a scripted stream-json peer: the conformance table of the design note
//! (section 6.3), each property with the case that would break it.

use super::*;
use std::fs;
use std::os::unix::fs::DirBuilderExt;

/// A stand-in for `claude -p` stream-json that follows the observed protocol: `queued`,
/// `started`, `init`, text, `result`, `completed`; `SLOW` runs a tool until an interrupt or a
/// mid-turn input arrives; `REPLY` runs an `agent-teams reply --file` command;
/// `FAKE_MODE=silent` never acknowledges; `FAKE_MODE=crash` exits on
/// the first input; `FAKE_MODE=queueonly` acknowledges but never starts; `FAKE_MODE=crashmid`
/// exits inside a `SLOW` turn. It appends its argv to `FAKE_ARGS_LOG` and records received inputs in
/// `FAKE_TRANSCRIPT` the way Claude Code's transcript does.
const FAKE_HARNESS: &str = r#"
import json, os, queue, sys, threading, time
args = sys.argv[1:]
if os.environ.get("FAKE_ARGS_LOG"):
    with open(os.environ["FAKE_ARGS_LOG"], "a") as log:
        log.write(json.dumps(args) + "\n")
mode = os.environ.get("FAKE_MODE", "normal")
session = args[args.index("--session-id") + 1] if "--session-id" in args else args[args.index("--resume") + 1]
transcript = os.environ.get("FAKE_TRANSCRIPT")
inputs = queue.Queue()
def reader():
    for line in sys.stdin:
        inputs.put(json.loads(line))
    inputs.put(None)
threading.Thread(target=reader, daemon=True).start()
def out(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()
def life(uuid, state):
    out({"type": "command_lifecycle", "command_uuid": uuid, "state": state})
def record(uuid):
    if transcript:
        os.makedirs(os.path.dirname(transcript), exist_ok=True)
        with open(transcript, "a") as handle:
            handle.write(json.dumps({"type": "user", "uuid": uuid}) + "\n")
def control(message):
    out({"type": "control_response", "response": {"subtype": "success",
         "request_id": message["request_id"], "response": {"still_queued": []}}})
def turn(first):
    batch = [first]
    life(first["uuid"], "started")
    record(first["uuid"])
    out({"type": "system", "subtype": "init", "claude_code_version": "9.9.9", "model": "fake",
         "permissionMode": "dontAsk", "session_id": session, "tools": ["Bash"]})
    if "REPLY" in first["message"]["content"]:
        out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_use", "id": "t2", "name": "Bash",
             "input": {"command": "agent-teams reply tm-1 --file reply.md"}}]}})
        out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t2",
             "content": "sent\nkey AbCdEfGh1234567890AbCdEfGh12345678\n3\n4\n5\n6\n7\n8\n9\n10\n"}]}})
    if "SLOW" in first["message"]["content"]:
        out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "sleep 30"}}]}})
        if mode == "crashmid":
            sys.exit(5)
        deadline = time.time() + 20
        interrupted = False
        while time.time() < deadline:
            try:
                message = inputs.get(timeout=0.05)
            except queue.Empty:
                continue
            if message is None:
                break
            if message.get("type") == "control_request":
                control(message)
                interrupted = True
                break
            life(message["uuid"], "queued")
            life(message["uuid"], "started")
            record(message["uuid"])
            batch.append(message)
            break
        if interrupted:
            out({"type": "user", "message": {"content": [{"type": "text", "text": "[Request interrupted by user]"}]}})
            out({"type": "result", "subtype": "error_during_execution", "usage": {}})
            for item in batch:
                life(item["uuid"], "cancelled")
            return
        out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "done"}]}})
    for item in batch:
        out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
            {"type": "text", "text": "echo: " + item["message"]["content"]}]}})
    out({"type": "result", "subtype": "success", "duration_ms": 5,
         "usage": {"input_tokens": 1, "output_tokens": 2},
         "model_usage_since_last_result": {"fake": {"costUSD": 0.001}}})
    for item in batch:
        life(item["uuid"], "completed")
while True:
    message = inputs.get()
    if message is None:
        break
    if message.get("type") == "control_request":
        control(message)
        continue
    if mode == "silent":
        continue
    if mode == "crash":
        sys.exit(3)
    life(message["uuid"], "queued")
    if mode == "queueonly":
        continue
    turn(message)
"#;

struct Fixture {
    root: PathBuf,
    agent_directory: PathBuf,
    config: WrapperConfig,
}

impl Fixture {
    fn new(label: &str, mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-wrap-{label}-{}-{}",
            std::process::id(),
            events::now_millis() % 1_000_000
        ));
        let agent_directory = root.join("reg").join("agent");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&agent_directory)
            .unwrap();
        let script = root.join("fake_claude.py");
        fs::write(&script, FAKE_HARNESS).unwrap();
        let cwd = root.join("work");
        fs::create_dir_all(&cwd).unwrap();
        let session = "6b4a1c1e-2f6e-4f7e-9a51-1d0c3e2a9b10";
        let mut config = WrapperConfig::new("agent", &agent_directory, &cwd, session);
        let claude_config = root.join("claude-config");
        config.command = vec![
            "env".to_owned(),
            format!("FAKE_MODE={mode}"),
            format!("FAKE_ARGS_LOG={}", root.join("args.log").display()),
            format!(
                "FAKE_TRANSCRIPT={}",
                claude::transcript_path(&cwd, session, &claude_config).display()
            ),
            "python3".to_owned(),
            script.display().to_string(),
        ];
        config.config_directory = Some(claude_config);
        config.ack_deadline = Duration::from_secs(10);
        config.startup_grace = Duration::ZERO;
        config.stop_timeout = Duration::from_secs(10);
        Self {
            root,
            agent_directory,
            config,
        }
    }

    fn queue(&self) -> PathBuf {
        self.agent_directory.join("queue")
    }

    fn enqueue(&self, id: &str, text: &str, extra: Value) {
        queue::enqueue_into(
            &self.queue(),
            queue::Lane::InboxV2,
            Some(id),
            true,
            &Blocking,
            |identifier| {
                let mut item = json!({"id": identifier, "text": text, "queued_at": unix_seconds(),
                                      "delivery_attempts": 0});
                if let (Some(item), Some(extra)) = (item.as_object_mut(), extra.as_object()) {
                    item.extend(extra.clone());
                }
                item
            },
        )
        .unwrap();
        self.request("wake");
    }

    /// Start a wrapper on a thread once it holds the lock. Another test of this binary that
    /// forks without exec can briefly inherit the previous wrapper's lock descriptor, so a start
    /// refused with [`EXIT_LOCKED`] is retried.
    fn start(&self) -> thread::JoinHandle<Result<i32>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let handle = loop {
            let config = self.config.clone();
            let handle =
                thread::spawn(move || run(config, Arc::new(AtomicBool::new(false)), &NoHooks));
            while !handle.is_finished() && !control::socket_path(&self.agent_directory).exists() {
                thread::sleep(Duration::from_millis(25));
            }
            if !handle.is_finished() {
                break handle;
            }
            let code = handle.join().unwrap().unwrap();
            assert!(
                code == EXIT_LOCKED && Instant::now() < deadline,
                "wrapper exited {code} at start"
            );
            thread::sleep(Duration::from_millis(200));
        };
        self.wait("socket", || {
            control::socket_path(&self.agent_directory).exists()
        });
        self.wait("idle", || {
            self.has_event("status", |data| data["status"] == "idle")
        });
        handle
    }

    fn request(&self, op: &str) -> Value {
        control::request(
            &control::socket_path(&self.agent_directory),
            &json!({"op": op}),
            Duration::from_secs(20),
        )
        .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}))
    }

    fn events(&self) -> Vec<events::Event> {
        events::read_all(&events::directory(&self.agent_directory)).unwrap()
    }

    fn has_event(&self, kind: &str, matches: impl Fn(&Value) -> bool) -> bool {
        self.events()
            .iter()
            .any(|event| event.kind == kind && matches(&event.data))
    }

    /// The item's phase; a rename in progress (the item briefly in two phases) is read again.
    fn state(&self, id: &str) -> Option<queue::MessageState> {
        for _ in 0..20 {
            if let Ok(state) = queue::message_state(&self.queue(), id) {
                return state;
            }
            thread::sleep(Duration::from_millis(5));
        }
        queue::message_state(&self.queue(), id).unwrap()
    }

    fn wait(&self, what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; events: {:#?}",
                self.events()
                    .iter()
                    .map(|e| (&e.kind, &e.data))
                    .collect::<Vec<_>>()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn stop(&self, handle: thread::JoinHandle<Result<i32>>) {
        assert_eq!(self.request("stop")["ok"], true);
        assert_eq!(handle.join().unwrap().unwrap(), 0);
        assert!(self.has_event("lifecycle.stopped", |data| data["cause"] == "stop-request"));
        assert!(!control::socket_path(&self.agent_directory).exists());
    }

    fn kinds(&self) -> Vec<String> {
        self.events().into_iter().map(|event| event.kind).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn position(kinds: &[String], kind: &str) -> usize {
    kinds
        .iter()
        .position(|value| value == kind)
        .unwrap_or_else(|| panic!("no {kind} in {kinds:?}"))
}

#[test]
fn an_item_settles_processed_only_after_started_and_the_turn_is_logged() {
    let fixture = Fixture::new("idle", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "hello there", json!({"from": "owner"}));
    fixture.wait("processed", || {
        fixture.state("m1") == Some(queue::MessageState::Processed)
    });
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let processed = queue::load_message(&fixture.queue().join("processed/m1.json")).unwrap();
    assert!(processed.contains_key("started_at"), "{processed:?}");
    assert!(processed.contains_key("harness_queued_at"), "{processed:?}");
    assert!(!processed.contains_key("paste_started_at"), "{processed:?}");
    assert_eq!(processed["mid_turn"], false);
    let kinds = fixture.kinds();
    assert!(position(&kinds, "input.queued") < position(&kinds, "input.started"));
    assert!(position(&kinds, "turn.started") < position(&kinds, "input.started"));
    assert!(position(&kinds, "input.started") < position(&kinds, "assistant.text"));
    assert!(position(&kinds, "assistant.text") < position(&kinds, "turn.completed"));
    assert!(
        fixture.has_event("input.queued", |data| data["from"] == "owner"
            && data["excerpt"] == "hello there"
            && data["deliver"] == "turn-boundary")
    );
    assert!(fixture.has_event("assistant.text", |data| data["text"] == "echo: hello there"));
    assert!(
        fixture.has_event("turn.completed", |data| data["outcome"] == "completed"
            && data["cost_usd"] == 0.001)
    );
    assert!(fixture.has_event("lifecycle.harness", |data| data["version"] == "9.9.9"));
    let status = fixture.request("status");
    assert_eq!(status["status"], "idle", "{status}");
    fixture.stop(handle);
}

#[test]
fn a_reply_command_is_logged_with_its_file_text_and_tool_output_is_previewed() {
    let fixture = Fixture::new("reply", "normal");
    fs::write(fixture.config.cwd.join("reply.md"), "## Done\n\n- landed\n").unwrap();
    let handle = fixture.start();
    fixture.enqueue("m1", "REPLY please", json!({"from": "owner"}));
    fixture.wait("reply.sent", || fixture.has_event("reply.sent", |_| true));
    assert!(fixture.has_event("tool.completed", |data| {
        data["output_lines"] == 10
            && data["preview"]
                .as_array()
                .is_some_and(|lines| lines.len() == claude::PREVIEW_LINES && lines[1] == "key ***")
    }));
    assert!(
        fixture.has_event("reply.sent", |data| data["channel"] == "team"
            && data["to"] == "tm-1"
            && data["text"] == "## Done\n\n- landed"
            && data["file"] == "reply.md")
    );
    let kinds = fixture.kinds();
    assert!(position(&kinds, "tool.completed") < position(&kinds, "reply.sent"));
    fixture.stop(handle);
}

#[test]
fn a_now_item_starts_mid_turn_and_a_turn_boundary_item_waits() {
    let fixture = Fixture::new("mid", "normal");
    let handle = fixture.start();
    fixture.enqueue("slow", "SLOW work", json!({}));
    fixture.wait("tool running", || {
        fixture.has_event("tool.started", |_| true)
    });
    fixture.enqueue("later", "after the turn", json!({}));
    fixture.enqueue("urgent", "owner question", json!({"priority": "urgent"}));
    fixture.wait("urgent processed", || {
        fixture.state("urgent") == Some(queue::MessageState::Processed)
    });
    fixture.wait("later processed", || {
        fixture.state("later") == Some(queue::MessageState::Processed)
    });
    fixture.wait("two turns", || {
        fixture
            .events()
            .iter()
            .filter(|event| event.kind == "turn.completed")
            .count()
            == 2
    });
    let events = fixture.events();
    let started = |id: &str| {
        events
            .iter()
            .find(|event| event.kind == "input.started" && event.data["queue_id"] == id)
            .unwrap()
            .clone()
    };
    let first_turn_end = events
        .iter()
        .find(|event| event.kind == "turn.completed")
        .unwrap()
        .seq;
    let urgent = started("urgent");
    assert_eq!(urgent.data["mid_turn"], true);
    assert!(urgent.seq < first_turn_end);
    assert_eq!(urgent.turn, started("slow").turn);
    let later = started("later");
    assert_eq!(later.data["mid_turn"], false);
    assert!(later.seq > first_turn_end);
    fixture.stop(handle);
}

#[test]
fn interrupt_ends_the_running_turn_and_the_next_input_runs() {
    let fixture = Fixture::new("interrupt", "normal");
    let handle = fixture.start();
    fixture.enqueue("slow", "SLOW work", json!({}));
    fixture.wait("tool running", || {
        fixture.has_event("tool.started", |_| true)
    });
    let answer = fixture.request("interrupt");
    assert_eq!(answer["ok"], true, "{answer}");
    fixture.wait("interrupted", || {
        fixture.has_event("turn.completed", |data| data["outcome"] == "interrupted")
    });
    assert_eq!(fixture.state("slow"), Some(queue::MessageState::Processed));
    fixture.wait("cancelled after start", || {
        fixture.has_event("input.cancelled", |data| data["before_start"] == false)
    });
    fixture.enqueue("next", "are you there", json!({}));
    fixture.wait("next processed", || {
        fixture.has_event("assistant.text", |data| {
            data["text"] == "echo: are you there"
        })
    });
    fixture.stop(handle);
}

#[test]
fn restart_resumes_the_session_and_recovery_settles_inflight_items() {
    let fixture = Fixture::new("resume", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "first", json!({}));
    fixture.wait("m1", || {
        fixture.state("m1") == Some(queue::MessageState::Processed)
    });
    fixture.stop(handle);
    let transcript = fixture.config.transcript().unwrap();
    // Three items a crashed wrapper left in inflight/: one the transcript proves, one written but
    // unproven, one provably never written.
    let inflight = fixture.queue().join("inflight");
    fs::write(&transcript, {
        let mut text = fs::read_to_string(&transcript).unwrap();
        text.push_str(
            &json!({"type": "user", "uuid": "0e0e0e0e-0000-4000-8000-000000000001"}).to_string(),
        );
        text.push('\n');
        text
    })
    .unwrap();
    for (id, stamps) in [
        (
            "proven",
            json!({"paste_tracking": true, "inflight_from": "inbox.v2", "paste_started_at": 1.0,
                          "harness_input_id": "0e0e0e0e-0000-4000-8000-000000000001"}),
        ),
        (
            "unproven",
            json!({"paste_tracking": true, "inflight_from": "inbox.v2", "paste_started_at": 1.0,
                            "harness_input_id": "0e0e0e0e-0000-4000-8000-000000000002"}),
        ),
        (
            "unwritten",
            json!({"paste_tracking": true, "inflight_from": "inbox.v2"}),
        ),
    ] {
        let mut item = json!({"id": id, "text": format!("item {id}"), "queued_at": 1.0,
                              "possibly_submitted": true});
        item.as_object_mut()
            .unwrap()
            .extend(stamps.as_object().unwrap().clone());
        atomic_json(&inflight.join(format!("{id}.json")), &item).unwrap();
    }
    let handle = fixture.start();
    fixture.wait("unwritten delivered", || {
        fixture.state("unwritten") == Some(queue::MessageState::Processed)
    });
    assert_eq!(
        fixture.state("proven"),
        Some(queue::MessageState::Processed)
    );
    let proven = queue::load_message(&fixture.queue().join("processed/proven.json")).unwrap();
    assert_eq!(proven["recovered"], "transcript");
    assert_eq!(fixture.state("unproven"), Some(queue::MessageState::Failed));
    fixture.stop(handle);
    let argv: Vec<Vec<String>> = fs::read_to_string(fixture.root.join("args.log"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(argv[0].contains(&"--session-id".to_owned()), "{argv:?}");
    assert!(argv[1].contains(&"--resume".to_owned()), "{argv:?}");
    assert!(argv[1].contains(&fixture.config.session), "{argv:?}");
    // Two clean stops: no restart notice was queued.
    assert!(fixture.state("restart-g2").is_none());
    let seqs: Vec<u64> = fixture.events().iter().map(|event| event.seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{seqs:?}"
    );
}

#[test]
fn a_harness_crash_after_writing_quarantines_the_item_and_restarts_with_resume() {
    let mut fixture = Fixture::new("crash", "crash");
    fixture.config.max_restarts = 1;
    let config = fixture.config.clone();
    let handle = thread::spawn(move || run(config, Arc::new(AtomicBool::new(false)), &NoHooks));
    fixture.wait("idle", || {
        fixture.has_event("status", |data| data["status"] == "idle")
    });
    fixture.enqueue("doomed", "anything", json!({}));
    fixture.wait("restarted", || {
        fixture
            .events()
            .iter()
            .filter(|event| event.kind == "lifecycle.harness_spawned")
            .count()
            == 2
    });
    fixture.wait("doomed quarantined", || {
        fixture.state("doomed") == Some(queue::MessageState::Failed)
    });
    fixture.enqueue("second", "anything", json!({}));
    assert_eq!(handle.join().unwrap().unwrap(), 1);
    assert!(fixture.has_event("lifecycle.stopped", |data| data["cause"] == "restart-limit"));
    // The wrapper that gave up left a restart notice for the next generation.
    let handle = fixture.start();
    fixture.wait("notice queued", || fixture.state("restart-g2").is_some());
    let _ = fixture.request("stop");
    let _ = handle.join();
}

#[test]
fn an_unacknowledged_input_is_quarantined_after_the_deadline() {
    let mut fixture = Fixture::new("silent", "silent");
    fixture.config.ack_deadline = Duration::from_millis(300);
    let handle = fixture.start();
    fixture.enqueue("lost", "hello", json!({}));
    fixture.wait("quarantined", || {
        fixture.queue().join("failed/lost.json.error").exists()
    });
    let sidecar = fs::read_to_string(fixture.queue().join("failed/lost.json.error")).unwrap();
    assert!(sidecar.contains("possibly_submitted"), "{sidecar}");
    fixture.stop(handle);
}

#[test]
fn a_second_wrapper_is_refused_while_one_runs_and_tombstoned_items_are_not_written() {
    let fixture = Fixture::new("lock", "normal");
    let handle = fixture.start();
    assert!(try_lock(&fixture.agent_directory).unwrap().is_none());
    let second = run(
        fixture.config.clone(),
        Arc::new(AtomicBool::new(false)),
        &NoHooks,
    )
    .unwrap();
    assert_eq!(second, EXIT_LOCKED);
    queue::enqueue_into(
        &fixture.queue(),
        queue::Lane::InboxV2,
        Some("tomb"),
        true,
        &Blocking,
        |identifier| json!({"id": identifier, "text": "x", "queued_at": unix_seconds(), "not_after": 1}),
    )
    .unwrap();
    fixture.request("wake");
    fixture.wait("set aside", || {
        fixture.state("tomb") == Some(queue::MessageState::Cancelled)
    });
    assert!(!fixture.has_event("input.queued", |data| data["queue_id"] == "tomb"));
    fixture.stop(handle);
}

#[test]
fn an_input_queued_but_never_started_by_an_idle_harness_is_quarantined() {
    let mut fixture = Fixture::new("queueonly", "queueonly");
    fixture.config.ack_deadline = Duration::from_millis(400);
    let handle = fixture.start();
    fixture.enqueue("stuck", "hello", json!({}));
    fixture.wait("quarantined", || {
        fixture.queue().join("failed/stuck.json.error").exists()
    });
    assert!(fixture.has_event("input.queued", |data| data["queue_id"] == "stuck"));
    // The next item is delivered: the wrapper is not wedged.
    fixture.enqueue("next", "again", json!({}));
    fixture.wait("next written", || {
        fixture.has_event("input.queued", |data| data["queue_id"] == "next")
    });
    fixture.stop(handle);
}

#[test]
fn a_harness_that_dies_inside_a_turn_is_restarted_and_the_agent_told() {
    let fixture = Fixture::new("crashmid", "crashmid");
    let handle = fixture.start();
    fixture.enqueue("slow", "SLOW work", json!({}));
    fixture.wait("restart notice delivered", || {
        fixture.has_event("assistant.text", |data| {
            data["text"]
                .as_str()
                .is_some_and(|text| text.contains("Your harness exited and was restarted"))
        })
    });
    assert_eq!(fixture.state("slow"), Some(queue::MessageState::Processed));
    let argv = fs::read_to_string(fixture.root.join("args.log")).unwrap();
    assert!(argv.lines().nth(1).unwrap().contains("--resume"), "{argv}");
    fixture.stop(handle);
}

#[test]
fn a_live_harness_of_an_earlier_wrapper_blocks_a_new_one() {
    let fixture = Fixture::new("orphan", "normal");
    // This test process stands in for a harness left running by a killed wrapper.
    let me = agent_queue::identity::observe(u64::from(std::process::id())).unwrap();
    atomic_json(
        &fixture.agent_directory.join(HARNESS_FILE),
        &json!({"schema": "agentctl-headless-harness/v1", "identity": me.identity}),
    )
    .unwrap();
    let code = run(
        fixture.config.clone(),
        Arc::new(AtomicBool::new(false)),
        &NoHooks,
    )
    .unwrap();
    assert_eq!(code, EXIT_LOCKED);
    assert!(fixture.events().is_empty());
}

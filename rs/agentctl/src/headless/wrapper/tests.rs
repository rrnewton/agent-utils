//! The wrapper against a scripted stream-json peer: the conformance table of the design note
//! (section 6.3), each property with the case that would break it.

use super::*;
use std::fs;
use std::os::unix::fs::DirBuilderExt;

/// A stand-in for `claude -p` stream-json that follows the observed protocol: `queued`,
/// `started`, `init`, text, `result`, `completed`; `SLOW` runs a tool until an interrupt or a
/// mid-turn input arrives; `REPLY` runs an `agent-teams reply --file` command; `ASK` runs a tool
/// call that raises a `can_use_tool` permission prompt and waits for its answer, as Claude Code
/// 2.1.310 does (`ASKLONG` with a command too long to show whole, `WITHDRAW` withdrawing the
/// prompt and running the call as a host hook allowed it, `SELFASK` also trying to answer it
/// itself through the control socket); `HOOKDENY` runs one a hook refuses outright;
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
def ask(denials, command="git push origin main", withdraw=False, selfask=False):
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
        {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": command}}]}})
    out({"type": "control_request", "request_id": "perm-1", "request": {"subtype": "can_use_tool",
         "tool_name": "Bash", "input": {"command": command}, "description": "Push",
         "decision_reason": "Pushes need a human.", "decision_reason_type": "hook", "tool_use_id": "t3"}})
    if withdraw:
        time.sleep(0.2)
        out({"type": "control_cancel_request", "request_id": "perm-1"})
        out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t3", "content": "ran"}]},
            "tool_result_meta": [{"id": "t3", "permission_decision": {"decision": "accept", "source": "hook"}}]})
        return
    if selfask:
        import socket
        client = socket.socket(socket.AF_UNIX)
        client.connect(os.environ["FAKE_SOCKET"])
        client.sendall((json.dumps({"op": "approval", "decision": "approve", "by": "console:me"}) + "\n").encode())
        with open(os.environ["FAKE_SELF_LOG"], "w") as log:
            log.write(client.makefile().readline())
    held = []
    answer = None
    deadline = time.time() + 25
    while time.time() < deadline and answer is None:
        try:
            message = inputs.get(timeout=0.05)
        except queue.Empty:
            continue
        if message is None:
            break
        if message.get("type") == "control_response" and message["response"]["request_id"] == "perm-1":
            answer = message["response"]["response"]
        else:
            held.append(message)
    for message in held:
        inputs.put(message)
    if answer and answer.get("behavior") == "allow":
        out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t3", "content": "pushed " + answer["updatedInput"]["command"][:40]}]},
            "tool_result_meta": [{"id": "t3", "permission_decision": {"decision": "accept", "source": "user_temporary"}}]})
    else:
        text = answer["message"] if answer else "no answer"
        out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
            {"type": "tool_result", "tool_use_id": "t3", "content": text, "is_error": True}]},
            "tool_result_meta": [{"id": "t3", "permission_decision": {"decision": "reject", "source": "user_reject"}}]})
        denials.append({"tool_name": "Bash", "tool_use_id": "t3", "tool_input": {"command": command}})
def hookdeny(denials):
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
        {"type": "tool_use", "id": "t4", "name": "Bash", "input": {"command": "cd /x && make"}}]}})
    out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
        {"type": "tool_result", "tool_use_id": "t4", "is_error": True,
         "content": "PreToolUse:Bash hook error: [hook]: Refused by the permission hook: never cd in a compound command.\nUse absolute paths."}]},
        "tool_result_meta": [{"id": "t4", "permission_decision": {"decision": "reject", "source": "hook"}}]})
    denials.append({"tool_name": "Bash", "tool_use_id": "t4", "tool_input": {"command": "cd /x && make"}})
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
    denials = []
    content = first["message"]["content"]
    if "ASKLONG" in content:
        ask(denials, command="echo " + "x" * 4000)
    elif "WITHDRAW" in content:
        ask(denials, withdraw=True)
    elif "SELFASK" in content:
        ask(denials, selfask=True)
    elif "ASK" in content:
        ask(denials)
    if "HOOKDENY" in first["message"]["content"]:
        hookdeny(denials)
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
         "usage": {"input_tokens": 1, "output_tokens": 2}, "permission_denials": denials,
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
        // A session of its own: a wrapper ends every process that names its session when its
        // harness exits, which would end a concurrent test's harness on a shared one.
        let session = claude::random_uuid().unwrap();
        let session = session.as_str();
        let mut config = WrapperConfig::new("agent", &agent_directory, &cwd, session);
        let claude_config = root.join("claude-config");
        config.command = vec![
            "env".to_owned(),
            format!("FAKE_MODE={mode}"),
            format!("FAKE_ARGS_LOG={}", root.join("args.log").display()),
            format!(
                "FAKE_SOCKET={}",
                control::socket_path(&agent_directory).display()
            ),
            format!("FAKE_SELF_LOG={}", root.join("self.log").display()),
            format!(
                "FAKE_TRANSCRIPT={}",
                claude::transcript_path(&cwd, session, &claude_config).display()
            ),
            "python3".to_owned(),
            script.display().to_string(),
        ];
        config.config_directory = Some(claude_config);
        // The test, its harness and its answers share one control group.
        config.approval_cgroup_check = false;
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

#[test]
fn a_takeover_generation_that_was_released_counts_as_a_clean_stop() {
    let root = std::env::temp_dir().join(format!(
        "agentctl-released-{}-{}",
        std::process::id(),
        events::now_millis()
    ));
    fs::create_dir_all(&root).unwrap();
    let dir = root.join("events");
    let mut writer = EventWriter::open(&dir, "a").unwrap();
    writer
        .append("lifecycle.takeover", None, json!({"tab": "t"}))
        .unwrap();
    drop(writer);
    assert!(
        previous_ended_uncleanly(&dir, u64::MAX),
        "a takeover that never ended is not clean"
    );
    let mut writer = EventWriter::open(&dir, "a").unwrap();
    writer
        .append("lifecycle.takeover", None, json!({"tab": "t"}))
        .unwrap();
    writer
        .append("lifecycle.released", None, json!({"code": 0}))
        .unwrap();
    drop(writer);
    assert!(!previous_ended_uncleanly(&dir, u64::MAX));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn held_deliveries_stay_queued_until_released() {
    let fixture = Fixture::new("hold", "normal");
    let handle = fixture.start();
    fixture.wait("idle", || fixture.request("status")["status"] == "idle");
    assert_eq!(fixture.request("hold")["held"], true);
    fixture.enqueue("m1", "hello", json!({"from": "owner"}));
    std::thread::sleep(Duration::from_millis(1500));
    assert_ne!(fixture.state("m1"), Some(queue::MessageState::Processed));
    assert_eq!(fixture.request("status")["held"], true);
    assert!(!fixture.has_event("input.queued", |_| true));
    assert_eq!(fixture.request("release")["held"], false);
    fixture.wait("processed", || {
        fixture.state("m1") == Some(queue::MessageState::Processed)
    });
    fixture.stop(handle);
}

impl Fixture {
    fn answer(&self, body: Value) -> Value {
        control::request(
            &control::socket_path(&self.agent_directory),
            &body,
            Duration::from_secs(20),
        )
        .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}))
    }

    /// The ID of the one `approval.requested` event, once logged.
    fn approval_id(&self) -> String {
        self.wait("approval.requested", || {
            self.has_event("approval.requested", |_| true)
        });
        self.events()
            .into_iter()
            .find(|event| event.kind == "approval.requested")
            .and_then(|event| event.data["id"].as_str().map(str::to_owned))
            .unwrap()
    }

    fn event_data(&self, kind: &str) -> Value {
        self.events()
            .into_iter()
            .find(|event| event.kind == kind)
            .map(|event| event.data)
            .unwrap_or_else(|| panic!("no {kind} event in {:?}", self.kinds()))
    }
}

#[test]
fn an_owner_approval_from_chat_lets_the_call_run_and_its_message_is_consumed() {
    let fixture = Fixture::new("approve", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK to push", json!({"from": "owner"}));
    let id = fixture.approval_id();
    let asked = fixture.event_data("approval.requested");
    assert_eq!(asked["tool"], "Bash");
    assert_eq!(asked["summary"], "git push origin main");
    assert_eq!(asked["reason"], "Pushes need a human.");
    assert_eq!(asked["reason_type"], "hook");
    assert_eq!(asked["timeout_s"], 600);
    let pending = fixture.request("approvals");
    assert_eq!(pending["approvals"][0]["id"], id.as_str(), "{pending}");
    assert_eq!(fixture.request("status")["approvals_pending"], 1);
    // The bridge passes the owner's answer first, then queues the chat message that carried it.
    let answer = fixture.answer(json!({"op": "approval", "id": id, "decision": "approve",
                                       "by": "chat:U1", "queue_id": "chat-k1"}));
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["approval"]["decision"], "approved");
    let again = fixture.answer(json!({"op": "approval", "id": id, "decision": "approve",
                                      "by": "chat:U1", "queue_id": "chat-k1"}));
    assert_eq!(again["already"], true, "{again}");
    fixture.enqueue(
        "chat-k1",
        &format!("Sent 1 s ago by the owner: approve {id}"),
        json!({"from": "agentctl", "priority": "urgent", "deliver": "now"}),
    );
    fixture.wait("the call ran", || {
        fixture.has_event("tool.completed", |data| {
            data["ok"] == true && data["preview"][0] == "pushed git push origin main"
        })
    });
    fixture.wait("consumed", || {
        fixture.has_event("input.consumed", |data| data["queue_id"] == "chat-k1")
    });
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let decided = fixture.event_data("approval.decided");
    assert_eq!(decided["decision"], "approved");
    assert_eq!(decided["by"], "chat:U1");
    let item = queue::load_message(&fixture.queue().join("processed/chat-k1.json")).unwrap();
    assert_eq!(item["consumed"], "approval");
    assert_eq!(item["approval_id"], id.as_str());
    // The harness never saw the answer as a prompt.
    assert!(!fixture.has_event("input.queued", |data| data["queue_id"] == "chat-k1"));
    assert!(!fixture.has_event("assistant.text", |data| data["text"]
        .as_str()
        .is_some_and(|text| text.contains("approve"))));
    let args = fs::read_to_string(fixture.root.join("args.log")).unwrap();
    assert!(
        args.contains(r#""--permission-prompts", "host", "--permission-prompt-tool", "stdio""#),
        "{args}"
    );
    assert!(fixture.has_event("lifecycle.started", |data| data["approvals"] == true));
    fixture.stop(handle);
}

#[test]
fn an_owner_denial_reaches_the_model_with_its_reason_and_is_logged_in_full() {
    let fixture = Fixture::new("deny", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK to push", json!({"from": "owner"}));
    let id = fixture.approval_id();
    // No ID answers the only pending prompt.
    let answer = fixture.answer(
        json!({"op": "approval", "decision": "deny", "by": "console:owner",
                                       "reason": "not now"}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let decided = fixture.event_data("approval.decided");
    assert_eq!(decided["decision"], "denied");
    assert_eq!(decided["by"], "console:owner");
    assert_eq!(decided["reason"], "not now");
    let completed = fixture.event_data("tool.completed");
    assert_eq!(completed["ok"], false);
    assert_eq!(completed["summary"], "git push origin main");
    assert_eq!(completed["denied"]["source"], "approval");
    assert_eq!(
        completed["denied"]["reason"],
        format!("The owner denied this call at the console (approval {id}): not now")
    );
    let denied = fixture.event_data("permission.denied");
    assert_eq!(denied["tool_use_id"], "t3");
    assert_eq!(denied["source"], "approval");
    assert_eq!(denied["reason"], completed["denied"]["reason"]);
    let none =
        fixture.answer(json!({"op": "approval", "decision": "approve", "by": "console:owner"}));
    assert_eq!(none["class"], "not_pending", "{none}");
    fixture.stop(handle);
}

#[test]
fn an_unanswered_prompt_is_denied_at_the_timeout() {
    let mut fixture = Fixture::new("timeout", "normal");
    fixture.config.approval_timeout = Duration::from_secs(1);
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK to push", json!({"from": "owner"}));
    let id = fixture.approval_id();
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let decided = fixture.event_data("approval.decided");
    assert_eq!(decided["decision"], "denied");
    assert_eq!(decided["by"], "timeout");
    assert!(decided["waited_ms"].as_u64().unwrap() >= 1000, "{decided}");
    let completed = fixture.event_data("tool.completed");
    let reason = completed["denied"]["reason"].as_str().unwrap();
    assert!(
        reason.contains(&format!("(approval {id}), and no answer came within 1 s")),
        "{reason}"
    );
    let late =
        fixture.answer(json!({"op": "approval", "id": id, "decision": "approve", "by": "chat:U1"}));
    assert_eq!(late["class"], "not_pending", "{late}");
    fixture.stop(handle);
}

#[test]
fn the_agent_cannot_answer_its_own_prompt_and_bad_answers_are_refused() {
    let fixture = Fixture::new("selfapprove", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK to push", json!({"from": "owner"}));
    let id = fixture.approval_id();
    // A process that runs as the agent (as its own tool calls do) is refused.
    let socket = control::socket_path(&fixture.agent_directory);
    let client = "import json, socket, sys\n\
        s = socket.socket(socket.AF_UNIX)\n\
        s.connect(sys.argv[1])\n\
        s.sendall((json.dumps({'op': 'approval', 'id': sys.argv[2], 'decision': 'approve', 'by': 'chat:U1'}) + '\\n').encode())\n\
        print(s.makefile().readline())\n";
    let output = std::process::Command::new("python3")
        .args(["-c", client])
        .arg(&socket)
        .arg(&id)
        .env("AGENTCTL_AGENT", "agent")
        .output()
        .unwrap();
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["class"], "refused", "{answer}");
    assert!(fixture.has_event("approval.refused", |data| data["by"] == "chat:U1"));
    for (body, class) in [
        (
            json!({"op": "approval", "decision": "yes", "by": "chat:U1"}),
            "invalid",
        ),
        (
            json!({"op": "approval", "decision": "approve", "by": "agent:x"}),
            "invalid",
        ),
        (
            json!({"op": "approval", "id": "a999999", "decision": "approve", "by": "chat:U1"}),
            "not_pending",
        ),
    ] {
        let answer = fixture.answer(body);
        assert_eq!(answer["class"], class, "{answer}");
    }
    // Still pending: nothing above answered it.
    assert_eq!(
        fixture.request("approvals")["approvals"][0]["id"],
        id.as_str()
    );
    assert!(!fixture.has_event("approval.decided", |_| true));
    let answer =
        fixture.answer(json!({"op": "approval", "id": id, "decision": "deny", "by": "chat:U1"}));
    assert_eq!(answer["ok"], true, "{answer}");
    fixture.stop(handle);
}

#[test]
fn a_hook_refusal_stays_a_refusal_and_keeps_its_whole_reason() {
    let fixture = Fixture::new("hookdeny", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "HOOKDENY build", json!({"from": "owner"}));
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    assert!(!fixture.has_event("approval.requested", |_| true));
    let completed = fixture.event_data("tool.completed");
    assert_eq!(completed["denied"]["source"], "hook");
    assert_eq!(
        completed["denied"]["reason"],
        "PreToolUse:Bash hook error: [hook]: Refused by the permission hook: never cd in a compound command.\nUse absolute paths."
    );
    assert_eq!(completed["summary"], "cd /x && make");
    let denied = fixture.event_data("permission.denied");
    assert_eq!(denied["source"], "hook");
    fixture.stop(handle);
}

#[test]
fn approvals_off_runs_the_harness_with_prompts_denied() {
    let mut fixture = Fixture::new("noapprove", "normal");
    fixture.config.approvals = false;
    let handle = fixture.start();
    fixture.enqueue("m1", "hello", json!({"from": "owner"}));
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let args = fs::read_to_string(fixture.root.join("args.log")).unwrap();
    assert!(args.contains(r#""--permission-prompts", "none""#), "{args}");
    assert!(!args.contains("--permission-prompt-tool"), "{args}");
    fixture.stop(handle);
}

#[test]
fn a_chat_answer_must_name_the_prompt_and_cannot_approve_a_call_not_shown_whole() {
    let fixture = Fixture::new("asklong", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "ASKLONG", json!({"from": "owner"}));
    let id = fixture.approval_id();
    let asked = fixture.event_data("approval.requested");
    assert_eq!(asked["complete"], false, "{asked}");
    assert!(asked["detail"].as_str().unwrap().len() <= claude::DETAIL_BYTES + 3);
    let bare = fixture.answer(json!({"op": "approval", "decision": "approve", "by": "chat:U1"}));
    assert_eq!(bare["class"], "invalid", "{bare}");
    let chat =
        fixture.answer(json!({"op": "approval", "id": id, "decision": "approve", "by": "chat:U1"}));
    assert_eq!(chat["class"], "incomplete", "{chat}");
    assert!(
        fixture.has_event("approval.refused", |data| data["class"] == "incomplete"
            && data["id"] == id.as_str()
            && data["by"] == "chat:U1")
    );
    // The owner at the console may approve it.
    let console =
        fixture.answer(json!({"op": "approval", "decision": "approve", "by": "console:owner"}));
    assert_eq!(console["ok"], true, "{console}");
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    assert!(fixture.has_event("tool.completed", |data| data["ok"] == true));
    fixture.stop(handle);
}

#[test]
fn a_prompt_the_harness_withdraws_ends_cancelled_and_nothing_is_answered() {
    let fixture = Fixture::new("withdraw", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "WITHDRAW", json!({"from": "owner"}));
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    let decided = fixture.event_data("approval.decided");
    assert_eq!(decided["decision"], "cancelled");
    assert_eq!(decided["by"], "harness");
    assert_eq!(fixture.request("approvals")["approvals"], json!([]));
    fixture.stop(handle);
}

#[test]
fn the_harness_and_its_children_cannot_answer_their_own_prompt() {
    let fixture = Fixture::new("selfask", "normal");
    let handle = fixture.start();
    fixture.enqueue("m1", "SELFASK", json!({"from": "owner"}));
    let id = fixture.approval_id();
    fixture.wait("the harness's own answer", || {
        fixture.root.join("self.log").exists()
            && fs::metadata(fixture.root.join("self.log")).unwrap().len() > 0
    });
    let own: Value =
        serde_json::from_str(&fs::read_to_string(fixture.root.join("self.log")).unwrap()).unwrap();
    assert_eq!(own["class"], "refused", "{own}");
    assert!(
        own["error"]
            .as_str()
            .unwrap()
            .contains("inside the agent's own harness"),
        "{own}"
    );
    assert_eq!(
        fixture.request("approvals")["approvals"][0]["id"],
        id.as_str()
    );
    let answer =
        fixture.answer(json!({"op": "approval", "id": id, "decision": "deny", "by": "chat:U1"}));
    assert_eq!(answer["ok"], true, "{answer}");
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    fixture.stop(handle);
}

#[test]
fn with_approvals_off_a_prompt_is_denied_at_once() {
    let mut fixture = Fixture::new("offask", "normal");
    fixture.config.approvals = false;
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK", json!({"from": "owner"}));
    fixture.wait("turn completed", || {
        fixture.has_event("turn.completed", |_| true)
    });
    assert!(!fixture.has_event("approval.requested", |_| true));
    let completed = fixture.event_data("tool.completed");
    assert_eq!(completed["denied"]["source"], "approval");
    assert!(completed["denied"]["reason"]
        .as_str()
        .unwrap()
        .contains("takes no approvals"));
    fixture.stop(handle);
}

#[test]
fn an_answer_from_the_harnesss_control_group_is_refused() {
    let mut fixture = Fixture::new("cgroup", "normal");
    fixture.config.approval_cgroup_check = true;
    let handle = fixture.start();
    fixture.enqueue("m1", "ASK", json!({"from": "owner"}));
    let id = fixture.approval_id();
    let harness = fixture.request("status")["harness_pid"].as_u64().unwrap() as u32;
    let shared = approval::cgroup_of(harness).is_some()
        && approval::cgroup_of(harness) == approval::cgroup_of(std::process::id());
    let answer =
        fixture.answer(json!({"op": "approval", "id": id, "decision": "approve", "by": "chat:U1"}));
    if shared {
        assert_eq!(answer["class"], "refused", "{answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("control group"),
            "{answer}"
        );
        let deny = fixture
            .answer(json!({"op": "approval", "id": id, "decision": "deny", "by": "chat:U1"}));
        assert_eq!(deny["class"], "refused");
    } else {
        assert_eq!(answer["ok"], true, "{answer}");
    }
    fixture.stop(handle);
}

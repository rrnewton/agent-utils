//! `chat reply` stores a reply of an open request and reports what happened through its exit
//! status: 0 when the request holds the reply, with one JSON object on standard output; 1 when the
//! reply is refused, or when the state cannot be read or written; 2 for a usage error, which
//! includes a reply file that cannot be used; and 75 when nothing was stored but the same command
//! can succeed later.
//!
//! This drives the real binary as a subprocess, so it checks what the process prints and the
//! status it exits with. Each run uses a scratch bridge state that holds one request, a scratch
//! registry, scratch home directories, and a Herdr executable that does not exist, since the
//! command must never need Herdr.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agentctl::chat_runtime::{BridgeConfiguration, BridgeState, ReactionSetting};
use chat_subscription::{
    ChannelId, CommittableEvent, DeliveryBatch, DeliveryId, EventSequence, InboundMessage,
    MessageId, ProviderCursor, SenderId, ThreadId,
};
use serde_json::{json, Value};

/// How long one run may take before it is killed. Each run ends at once, so this bounds a hang.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// A scratch directory holding a bridge state with one open request from the owner, which no
/// service runs on.
struct Scratch {
    root: PathBuf,
    state: PathBuf,
    key: String,
    reply_id: String,
}

/// The exit code and output of one run.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-reply-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).expect("create scratch directory");
        let state = root.join("state");
        fs::create_dir(&state).expect("create state directory");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).expect("private state");
        let bridge = BridgeState::initialize(
            &state,
            BridgeConfiguration {
                unknown: Default::default(),
                subscription_plugin: "fixture".to_owned(),
                subscription_environment: Vec::new(),
                channel_ids: vec!["spaces/example".to_owned()],
                allowed_senders: vec!["users/owner".to_owned()],
                max_grant_seconds: agentctl::chat_runtime::DEFAULT_MAX_GRANT_SECONDS,
                agent_name: "worker".to_owned(),
                agent_label: "worker".to_owned(),
                outbound_enabled: true,
                ack_reaction: ReactionSetting::Disabled,
                receipt_reaction: ReactionSetting::Inherit,
                reject_reaction: ReactionSetting::Inherit,
                backend_configuration: None,
                outbound_command: None,
                reply_alias_range: None,
                slack_format: None,
                slack_format_by_channel: std::collections::BTreeMap::new(),
                labels: None,
            },
        )
        .expect("initialize state");
        let message = InboundMessage::new(
            ChannelId::new("spaces/example").expect("channel"),
            MessageId::new("spaces/example/messages/one").expect("message"),
            ThreadId::new("spaces/example/threads/one").expect("thread"),
            SenderId::new("users/owner").expect("sender"),
            "request",
            "2026-09-30T12:00:00Z",
            false,
        )
        .expect("inbound message");
        let batch = DeliveryBatch::new(
            EventSequence::new(1).expect("sequence"),
            ProviderCursor::new("cursor").expect("cursor"),
            DeliveryId::new("delivery").expect("delivery"),
            vec![CommittableEvent::message_created(message)],
        )
        .expect("delivery batch");
        let key = bridge
            .admit_batch(&batch)
            .expect("admit request")
            .new_request_keys
            .remove(0);
        let reply_id = bridge
            .next_reply_route(&key)
            .expect("reply route")
            .expect("open request")
            .identifier;
        Self {
            root,
            state,
            key,
            reply_id,
        }
    }

    /// Write `contents` to the file `name` in the scratch directory, and return its path.
    fn file(&self, name: &str, contents: &[u8]) -> String {
        let path = self.root.join(name);
        fs::write(&path, contents).expect("write reply file");
        path.to_str().expect("UTF-8 path").to_owned()
    }

    /// Run `agentctl chat reply --bridge-state <state>` followed by `arguments`.
    fn reply(&self, arguments: &[&str]) -> Run {
        let state = self.state.to_str().expect("UTF-8 state").to_owned();
        let mut words = vec!["reply", "--bridge-state", state.as_str()];
        words.extend_from_slice(arguments);
        self.chat(&words, &[])
    }

    /// Run `agentctl chat` followed by `arguments`, with `environment` added to the scratch
    /// environment, which sets no bridge index unless `environment` does.
    fn chat(&self, arguments: &[&str], environment: &[(&str, &Path)]) -> Run {
        let home = self.root.join("home");
        let registry = self.root.join("registry");
        let registry_existed = registry.exists();
        let stdout_path = self.root.join("stdout");
        let stderr_path = self.root.join("stderr");
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("XDG_RUNTIME_DIR", home.join("runtime"))
            .arg("--registry")
            .arg(&registry)
            .env_remove("AGENTCTL_AGENT")
            .env_remove("AGENTCTL_CHAT_BRIDGE")
            .env("AGENTCTL_CHAT_BRIDGE_INDEX", "off")
            .envs(environment.iter().copied())
            .arg("--herdr-bin")
            .arg(self.root.join("absent-herdr"))
            .arg("chat")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).expect("create standard output"))
            .stderr(fs::File::create(&stderr_path).expect("create standard error"))
            .spawn()
            .expect("run agentctl");
        let status = wait_at_most(&mut child, RUN_LIMIT);
        let read = |path: &Path| {
            String::from_utf8(fs::read(path).expect("read output")).expect("UTF-8 output")
        };
        let (stdout, stderr) = (read(&stdout_path), read(&stderr_path));
        let status = status.unwrap_or_else(|| {
            panic!("chat {arguments:?} ran longer than {RUN_LIMIT:?} and was killed: {stderr}")
        });
        assert!(
            registry_existed || !registry.exists(),
            "chat {arguments:?} created a registry"
        );
        Run {
            code: status.code(),
            stdout,
            stderr,
        }
    }

    /// Run `chat reply` for this state's request with `reply_id` and the reply file `file`.
    fn reply_with(&self, reply_id: &str, file: &str) -> Run {
        self.reply(&[
            "--request",
            &self.key,
            "--reply-id",
            reply_id,
            "--file",
            file,
        ])
    }

    /// The JSON object that a run storing a reply of this state's request prints.
    fn result(&self, outcome: &str, ordinal: u32) -> Value {
        json!({
            "request": self.key,
            "reply_id": self.reply_id,
            "outcome": outcome,
            "ordinal": ordinal,
            "phase": "pending",
            "service_woken": false,
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Wait at most `limit` for `child` to exit. Past that, kill it and return `None`.
fn wait_at_most(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll agentctl") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Check that `run` printed one JSON object equal to `expected` and nothing on standard error.
fn assert_stored(run: &Run, expected: &Value) {
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stderr, "");
    let printed: Value = serde_json::from_str(&run.stdout).expect("one JSON object");
    assert_eq!(&printed, expected);
}

/// Check that `run` exited `code`, printed nothing on standard output, and named `reason` on
/// standard error.
fn assert_failed(run: &Run, code: i32, reason: &str) {
    assert_eq!(run.code, Some(code), "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains(reason), "{}", run.stderr);
}

#[test]
fn chat_reply_exits_0_with_the_stored_reply_and_stores_a_text_once() {
    let scratch = Scratch::new("stored");
    let reply = scratch.file("reply.md", b"answer\n");
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &reply),
        &scratch.result("stored", 1),
    );
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &reply),
        &scratch.result("already_stored", 1),
    );
    // The agent label that the service puts before every reply it sends counts toward the
    // 30,000 bytes.
    let longest = scratch.file(
        "longest.md",
        "x".repeat(30_000 - "[worker] ".len()).as_bytes(),
    );
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &longest),
        &scratch.result("stored", 2),
    );
}

#[test]
fn chat_reply_exits_1_for_a_refused_reply_and_2_for_an_unusable_file() {
    let scratch = Scratch::new("refused");
    let reply = scratch.file("reply.md", b"answer\n");
    assert_failed(
        &scratch.reply_with("anything", &reply),
        1,
        "the reply ID is not one that this request's prompt gives",
    );
    let unlabelled = scratch.file("unlabelled.md", "x".repeat(30_000).as_bytes());
    assert_failed(
        &scratch.reply_with(&scratch.reply_id, &unlabelled),
        1,
        "agent-labelled chat reply exceeds 30000 UTF-8 bytes",
    );
    assert_failed(
        &scratch.reply(&[
            "--request",
            "NOT-A-KEY",
            "--reply-id",
            &scratch.reply_id,
            "--file",
            &reply,
        ]),
        1,
        "chat request key must be 64 lowercase hexadecimal characters",
    );
    assert_failed(
        &scratch.reply(&["--request", &scratch.key, "--reply-id", &scratch.reply_id]),
        2,
        "--file",
    );
    let absent = scratch.root.join("absent.md");
    assert_failed(
        &scratch.reply_with(&scratch.reply_id, absent.to_str().expect("UTF-8 path")),
        2,
        "cannot read",
    );
    // An empty, blank or over-long text is a refused reply; a file that is not UTF-8 is unusable.
    for (name, contents, status, reason) in [
        (
            "empty.md",
            b"".to_vec(),
            1,
            "chat reply body must be nonempty",
        ),
        (
            "blank.md",
            b" \n\n".to_vec(),
            1,
            "chat reply body must be nonempty",
        ),
        (
            "long.md",
            format!("{}\n", "x".repeat(30_000)).into_bytes(),
            1,
            "chat reply exceeds 30000 UTF-8 bytes",
        ),
        (
            "binary.md",
            b"answer \xff\n".to_vec(),
            2,
            "does not contain valid UTF-8",
        ),
    ] {
        let file = scratch.file(name, &contents);
        assert_failed(
            &scratch.reply_with(&scratch.reply_id, &file),
            status,
            reason,
        );
    }
    // None of those stored anything.
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &reply),
        &scratch.result("stored", 1),
    );
}

#[test]
fn chat_reply_exits_75_when_a_short_reply_id_waits_for_an_unusable_alias_record() {
    let scratch = Scratch::new("try-again");
    let reply = scratch.file("reply.md", b"answer\n");
    fs::write(scratch.state.join("reply-aliases.json"), "{").expect("spoil the alias record");
    assert_failed(&scratch.reply_with("001", &reply), 75, "is unusable");
    // Nothing was stored, and a long reply ID does not depend on the record.
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &reply),
        &scratch.result("stored", 1),
    );
}

/// An index entry naming `scratch`'s state, in a fresh index directory under the scratch root,
/// as `chat run` writes one when it starts (#707 chat-prompt-anatomy).
fn index_naming(scratch: &Scratch, states: &[&Path]) -> PathBuf {
    let index = scratch.root.join("index");
    fs::create_dir_all(&index).expect("index directory");
    for (number, state) in states.iter().enumerate() {
        fs::write(
            index.join(format!("{number:016x}.json")),
            serde_json::to_vec(&json!({
                "schema": agentctl::chat_lookup::INDEX_SCHEMA,
                "state": fs::canonicalize(state).expect("canonical state"),
                "registry": scratch.root.join("registry"),
                "updated_at_millis": 1,
            }))
            .expect("entry JSON"),
        )
        .expect("write index entry");
    }
    index
}

#[test]
fn a_reply_id_alone_finds_the_bridge_and_its_request_through_the_index() {
    let scratch = Scratch::new("lookup-index");
    let reply = scratch.file("reply.md", b"answer\n");
    let index = index_naming(&scratch, &[&scratch.state]);
    let worker = Path::new("worker");
    let environment = [
        ("AGENTCTL_CHAT_BRIDGE_INDEX", index.as_path()),
        ("AGENTCTL_AGENT", worker),
    ];
    // The short form a prompt prints, and the same with the ID in --reply-id.
    assert_stored(
        &scratch.chat(
            &["reply", &scratch.reply_id, "--file", &reply],
            &environment,
        ),
        &scratch.result("stored", 1),
    );
    assert_stored(
        &scratch.chat(
            &["reply", "--reply-id", &scratch.reply_id, "--file", &reply],
            &environment[..1],
        ),
        &scratch.result("already_stored", 1),
    );
    // `chat sent`, `chat thread` and `chat help` take the same ID.
    let sent = scratch.chat(&["sent", &scratch.reply_id], &environment);
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
    assert!(sent.stdout.contains(&scratch.key), "{}", sent.stdout);
    let thread = scratch.chat(&["thread", &scratch.reply_id], &environment);
    assert_eq!(thread.code, Some(0), "{}", thread.stderr);
    assert!(thread.stdout.contains("> request"), "{}", thread.stdout);
    let help = scratch.chat(&["help", &scratch.reply_id], &environment);
    assert_eq!(help.code, Some(0), "{}", help.stderr);
    assert!(
        help.stdout
            .contains(&format!("chat reply {} --file FILE", scratch.reply_id)),
        "{}",
        help.stdout
    );
    assert!(
        help.stdout.contains(&format!("--request {}", scratch.key)),
        "{}",
        help.stdout
    );
    // An ID no bridge gave is refused, naming what was considered; help says so and exits 0.
    let unknown = scratch.chat(&["reply", "999", "--file", &reply], &environment);
    assert_failed(&unknown, 1, "gave the reply ID \"999\"");
    let help = scratch.chat(&["help", "999"], &environment);
    assert_eq!(help.code, Some(0), "{}", help.stderr);
    assert!(
        help.stdout
            .starts_with("agentctl could not find the bridge for reply ID 999"),
        "{}",
        help.stdout
    );
    // A chat subcommand's name prints that subcommand's help, as clap's help subcommand did.
    let reply_help = scratch.chat(&["help", "reply"], &[]);
    assert_eq!(reply_help.code, Some(0), "{}", reply_help.stderr);
    assert!(
        reply_help.stdout.contains("--reply-id"),
        "{}",
        reply_help.stdout
    );
    // With nothing registered there is no bridge to find.
    assert_failed(
        &scratch.chat(&["reply", &scratch.reply_id, "--file", &reply], &[]),
        1,
        "no chat bridge is registered for discovery",
    );
}

#[test]
fn a_bridge_targeting_the_caller_is_chosen_and_an_ambiguous_one_is_refused() {
    let first = Scratch::new("lookup-first");
    let second = Scratch::new("lookup-second");
    let reply = first.file("reply.md", b"answer\n");
    let index = index_naming(&first, &[&first.state, &second.state]);
    // Both bridges target `worker` and know only their own long IDs, so the ID picks one.
    let environment = [("AGENTCTL_CHAT_BRIDGE_INDEX", index.as_path())];
    assert_stored(
        &first.chat(&["reply", &first.reply_id, "--file", &reply], &environment),
        &first.result("stored", 1),
    );
    // Without an ID, two bridges fit and the command names both.
    let sent = first.chat(&["sent"], &environment);
    assert_eq!(sent.code, Some(1), "{}", sent.stdout);
    assert!(
        sent.stderr.contains("2 chat bridges fit"),
        "{}",
        sent.stderr
    );
    // --bridge-state still picks one explicitly.
    let state = first.state.to_str().expect("UTF-8 state");
    let sent = first.chat(&["sent", "--bridge-state", state], &environment);
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
    // AGENTCTL_CHAT_BRIDGE names one for every command.
    let sent = first.chat(
        &["sent"],
        &[("AGENTCTL_CHAT_BRIDGE", first.state.as_path())],
    );
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
}

#[test]
fn a_bridge_under_the_registrys_bridges_directory_is_found_without_the_index() {
    let scratch = Scratch::new("lookup-registry");
    let reply = scratch.file("reply.md", b"answer\n");
    let bridges = scratch
        .root
        .join("registry")
        .join(agentctl::chat_lookup::BRIDGES);
    fs::create_dir_all(&bridges).expect("bridges directory");
    std::os::unix::fs::symlink(&scratch.state, bridges.join("worker")).expect("link the state");
    assert_stored(
        &scratch.chat(&["reply", &scratch.reply_id, "--file", &reply], &[]),
        &scratch.result("stored", 1),
    );
    // --bridge NAME names it too.
    let sent = scratch.chat(&["sent", "--bridge", "worker"], &[]);
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
    assert!(sent.stdout.contains(&scratch.key), "{}", sent.stdout);
}

#[test]
fn a_known_caller_never_reaches_a_bridge_that_targets_another_agent() {
    let scratch = Scratch::new("lookup-other-agent");
    let reply = scratch.file("reply.md", b"answer\n");
    let index = index_naming(&scratch, &[&scratch.state]);
    // The only bridge targets `worker`, and gave this ID; an agent known as `reviewer` is
    // refused rather than replying through it.
    let reviewer = Path::new("reviewer");
    assert_failed(
        &scratch.chat(
            &["reply", &scratch.reply_id, "--file", &reply],
            &[
                ("AGENTCTL_CHAT_BRIDGE_INDEX", index.as_path()),
                ("AGENTCTL_AGENT", reviewer),
            ],
        ),
        1,
        "no chat bridge targets you (reviewer)",
    );
}

#[test]
fn migrate_state_moves_the_state_into_the_registry_and_the_old_path_keeps_working() {
    let scratch = Scratch::new("migrate-state");
    let reply = scratch.file("reply.md", b"answer\n");
    let state = scratch.state.to_str().expect("UTF-8 state").to_owned();
    let target = scratch.root.join("registry").join("bridges").join("worker");
    let report =
        |run: &Run| -> Value { serde_json::from_str(&run.stdout).expect("one JSON object") };
    // Without a registry there is nowhere to move it.
    let refused = scratch.chat(&["migrate-state", "--bridge-state", &state], &[]);
    assert_eq!(refused.code, Some(1), "{}", refused.stdout);
    assert!(report(&refused)["refused"]
        .as_str()
        .is_some_and(|reason| reason.contains("does not exist")));
    fs::create_dir(scratch.root.join("registry")).expect("registry");
    // A dry run prints the plan and moves nothing.
    let plan = scratch.chat(&["migrate-state", "--bridge-state", &state], &[]);
    assert_eq!(plan.code, Some(0), "{}{}", plan.stdout, plan.stderr);
    let printed = report(&plan);
    assert_eq!(printed["applied"], json!(false));
    assert_eq!(printed["target"], json!(target));
    assert!(scratch.state.is_dir() && !target.exists());
    // Refused while a `chat run` holds the state.
    {
        let bridge = BridgeState::open(&scratch.state).expect("open state");
        let _lease = bridge.acquire_runner_lease().expect("runner lease");
        let held = scratch.chat(&["migrate-state", "--bridge-state", &state, "--apply"], &[]);
        assert_eq!(held.code, Some(1), "{}", held.stdout);
        assert!(report(&held)["refused"]
            .as_str()
            .is_some_and(|reason| reason.contains("stop the bridge")));
        assert!(!target.exists());
    }
    let index = scratch.root.join("index");
    let applied = scratch.chat(
        &["migrate-state", "--bridge-state", &state, "--apply"],
        &[("AGENTCTL_CHAT_BRIDGE_INDEX", index.as_path())],
    );
    assert_eq!(
        applied.code,
        Some(0),
        "{}{}",
        applied.stdout,
        applied.stderr
    );
    assert_eq!(report(&applied)["applied"], json!(true));
    assert!(fs::symlink_metadata(&scratch.state)
        .expect("old path")
        .file_type()
        .is_symlink());
    assert!(target.join("bridge.json").is_file());
    let mode = fs::metadata(target.parent().expect("bridges"))
        .expect("bridges directory")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "REGISTRY/bridges is private");
    // The old path's command, as an older prompt printed it, still stores the reply.
    assert_stored(
        &scratch.reply_with(&scratch.reply_id, &reply),
        &scratch.result("stored", 1),
    );
    // --bridge NAME names the new place, and lookup finds it through the index.
    let sent = scratch.chat(&["sent", "--bridge", "worker"], &[]);
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
    assert!(sent.stdout.contains(&scratch.key), "{}", sent.stdout);
    assert_stored(
        &scratch.chat(
            &["reply", &scratch.reply_id, "--file", &reply],
            &[("AGENTCTL_CHAT_BRIDGE_INDEX", index.as_path())],
        ),
        &scratch.result("already_stored", 1),
    );
    let status = scratch.chat(&["status", "--bridge", "worker"], &[]);
    assert_eq!(status.code, Some(0), "{}", status.stderr);
    // Running it again says it is done.
    let again = scratch.chat(&["migrate-state", "--bridge-state", &state, "--apply"], &[]);
    assert_eq!(again.code, Some(0), "{}", again.stdout);
    assert_eq!(report(&again)["already_migrated"], json!(true));
}

#[test]
fn migrate_state_never_replaces_an_existing_bridge() {
    let scratch = Scratch::new("migrate-state-taken");
    let state = scratch.state.to_str().expect("UTF-8 state").to_owned();
    let taken = scratch.root.join("registry").join("bridges").join("worker");
    fs::create_dir_all(&taken).expect("an existing bridge");
    let refused = scratch.chat(&["migrate-state", "--bridge-state", &state, "--apply"], &[]);
    assert_eq!(refused.code, Some(1), "{}", refused.stdout);
    assert!(
        refused.stdout.contains("already exists"),
        "{}",
        refused.stdout
    );
    assert!(scratch.state.is_dir());
    // Another name is free.
    let moved = scratch.chat(
        &[
            "migrate-state",
            "--bridge-state",
            &state,
            "--name",
            "worker-2",
            "--apply",
        ],
        &[],
    );
    assert_eq!(moved.code, Some(0), "{}{}", moved.stdout, moved.stderr);
    assert!(taken
        .with_file_name("worker-2")
        .join("bridge.json")
        .is_file());
}

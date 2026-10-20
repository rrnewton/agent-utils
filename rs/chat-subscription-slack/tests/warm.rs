//! The warm worker and the one-shot helper as the bridge runs them, against a fake Slack client
//! script that records every client process it runs: which process posted, what body it read,
//! and whether a discarded spare posted anything.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const APP: &str = "A0TESTAPP01";
const THREAD: &str = "1792000100.000100";

/// A fake client. Each `slack.message send` logs `start PID ARGS`, reads its body, refuses an
/// empty one as the real client does (unless `post-empty` exists: then, like a client that would
/// post whatever it read, it waits a second, as posting takes time, and posts even an empty body), fails as not applied once if `fail-next-send` exists, and
/// otherwise logs `posted PID` and prints a receipt whose ts ends in its PID modulo 1,000,000.
const FAKE_CLIENT: &str = r#"#!/bin/sh
printf 'start %s %s\n' "$$" "$*" >> "$FAKE_SLACK_DIR/argv.log"
case "$1 $2" in
  "slack.message send")
    cat > "$FAKE_SLACK_DIR/body-$$.txt"
    if [ -e "$FAKE_SLACK_DIR/post-empty" ]; then
      sleep 1
    elif [ ! -s "$FAKE_SLACK_DIR/body-$$.txt" ]; then
      printf '%s\n' '{"status":"error","message":"--markdown must not be empty."}'
      exit 1
    fi
    if [ -e "$FAKE_SLACK_DIR/fail-next-send" ]; then
      rm -f "$FAKE_SLACK_DIR/fail-next-send"
      printf '%s\n' '{"status":"error","message":"invalid_auth: the token expired"}'
      exit 1
    fi
    printf 'posted %s\n' "$$" >> "$FAKE_SLACK_DIR/argv.log"
    printf '{"data":{"channel_id":"C0C91RW5FAR","message_ts":"1792000999.%06d"}}\n' "$(( $$ % 1000000 ))" ;;
  "slack.reaction add")
    printf '%s\n' '{"data":{"changed":true}}' ;;
  *)
    printf '%s\n' '{"status":"error","message":"unexpected command"}'
    exit 1 ;;
esac
"#;

/// A private directory under the temporary directory, removed when the test passes.
struct Workspace(PathBuf);

impl std::ops::Deref for Workspace {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if !thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn workspace(name: &str) -> Workspace {
    Workspace(fresh_workspace(name))
}

fn fresh_workspace(name: &str) -> PathBuf {
    // Short, so the socket path fits in a Unix socket address.
    let root = std::env::temp_dir().join(format!("sw-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let client = root.join("slack-cli");
    fs::write(&client, FAKE_CLIENT).unwrap();
    fs::set_permissions(&client, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn log_lines(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("argv.log"))
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The PIDs of the client runs that started a send, and of those that posted.
fn sends(root: &Path) -> (Vec<String>, Vec<String>) {
    let lines = log_lines(root);
    let started = lines
        .iter()
        .filter(|line| line.contains("slack.message send"))
        .map(|line| line.split(' ').nth(1).unwrap().to_owned())
        .collect();
    let posted = lines
        .iter()
        .filter_map(|line| line.strip_prefix("posted "))
        .map(str::to_owned)
        .collect();
    (started, posted)
}

fn common_arguments(root: &Path, app: &str) -> Vec<String> {
    vec![
        format!("--cli={}", root.join("slack-cli").display()),
        format!("--app-id={app}"),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", root.join("ledger").display()),
        format!("--warm-socket={}", root.join("w.sock").display()),
    ]
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_worker(root: &Path, extra: &[&str]) -> Worker {
    let mut arguments = common_arguments(root, APP);
    arguments.push("--role=warm-worker".to_owned());
    arguments.push(format!(
        "--diagnostics-log={}",
        root.join("worker.log").display()
    ));
    arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));
    let child = Command::new(env!("CARGO_BIN_EXE_agentctl-slack-outbound"))
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("FAKE_SLACK_DIR", root)
        .spawn()
        .expect("worker starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("w.sock").exists() {
        assert!(
            Instant::now() < deadline,
            "the worker never bound its socket"
        );
        thread::sleep(Duration::from_millis(10));
    }
    Worker(child)
}

fn run_helper(root: &Path, app: &str, request: &Value) -> Value {
    let mut arguments = common_arguments(root, app);
    arguments.push(format!(
        "--diagnostics-log={}",
        root.join("helper.log").display()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl-slack-outbound"))
        .args(&arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("FAKE_SLACK_DIR", root)
        .spawn()
        .expect("helper starts");
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    child.stdin.take().unwrap().write_all(&line).unwrap();
    let output = child.wait_with_output().expect("helper exits");
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).expect("one JSON response")
}

fn reaction(id: &str, ts: &str) -> Value {
    json!({
        "version": 1, "id": id, "action": "ensure_reaction", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/{ts}"), "emoji": "👀",
    })
}

fn send(id: &str, thread: &str, text: &str) -> Value {
    json!({
        "version": 1, "id": id, "action": "send", "channel_id": CHANNEL,
        "thread_id": format!("{CHANNEL}/threads/{thread}"), "text": text,
    })
}

const ID1: &str = "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
const ID2: &str = "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b";

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: &str) -> bool {
    // A reaped process has no /proc entry; a zombie would still read as present, but the worker
    // reaps every spare it discards.
    Path::new(&format!("/proc/{pid}")).exists()
}

#[test]
fn a_reply_goes_through_the_spare_that_the_reaction_started() {
    let root = workspace("reply");
    let _worker = start_worker(&root, &[]);
    let acknowledged = run_helper(&root, APP, &reaction(ID1, THREAD));
    assert_eq!(acknowledged["ok"], true, "{acknowledged}");
    // The worker started one client for a reply in the message's thread, which now waits.
    wait_for("the spare to start", || sends(&root).0.len() == 1);
    let (started, posted) = sends(&root);
    assert!(posted.is_empty());
    let spare = &started[0];
    let start_line = log_lines(&root)
        .into_iter()
        .find(|line| line.starts_with(&format!("start {spare} ")))
        .unwrap();
    assert!(
        start_line.contains(&format!("--thread-ts={THREAD}")),
        "{start_line}"
    );
    assert!(start_line.contains(&format!("--as={APP}")), "{start_line}");

    let replied = run_helper(&root, APP, &send(ID2, THREAD, "the answer"));
    assert_eq!(replied["ok"], true, "{replied}");
    let (started, posted) = sends(&root);
    assert_eq!(started, std::slice::from_ref(spare), "no second client run");
    assert_eq!(posted, std::slice::from_ref(spare));
    assert_eq!(
        replied["receipt"]["message_id"],
        format!(
            "{CHANNEL}/messages/1792000999.{:06}",
            spare.parse::<u64>().unwrap() % 1_000_000
        )
    );
    assert_eq!(
        fs::read_to_string(root.join(format!("body-{spare}.txt"))).unwrap(),
        "the answer"
    );
    // The ledger recorded it: a retry of the same operation posts nothing.
    let again = run_helper(&root, APP, &send(ID2, THREAD, "the answer"));
    assert_eq!(again["receipt"], replied["receipt"]);
    assert_eq!(sends(&root).0.len(), 1);
}

#[test]
fn without_a_spare_the_helper_posts_with_its_own_client_run() {
    let root = workspace("cold");
    let _worker = start_worker(&root, &[]);
    let replied = run_helper(&root, APP, &send(ID1, THREAD, "cold answer"));
    assert_eq!(replied["ok"], true, "{replied}");
    let (started, posted) = sends(&root);
    assert_eq!(started.len(), 1);
    assert_eq!(posted, started);
    // A helper whose app differs from the worker's gets no spare either: its reaction starts
    // none, and its reply is its own client run.
    let other = "A0OTHERAPP1";
    run_helper(&root, other, &reaction(ID2, "1792000200.000100"));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(sends(&root).0.len(), 1, "no spare for another app");
}

#[test]
fn a_spare_that_fails_as_not_applied_is_replaced_by_a_new_client_run() {
    let root = workspace("fallback");
    let _worker = start_worker(&root, &[]);
    run_helper(&root, APP, &reaction(ID1, THREAD));
    wait_for("the spare to start", || sends(&root).0.len() == 1);
    fs::write(root.join("fail-next-send"), "").unwrap();
    let replied = run_helper(&root, APP, &send(ID2, THREAD, "second try"));
    assert_eq!(replied["ok"], true, "{replied}");
    let (started, posted) = sends(&root);
    assert_eq!(started.len(), 2, "the spare, then the helper's own run");
    assert_eq!(posted, [started[1].clone()], "only the second run posted");
}

#[test]
fn an_unused_spare_expires_and_posts_nothing() {
    let root = workspace("expiry");
    // This client would post even an empty body, a second after its input ends, so only killing
    // the spare, not merely closing its input, keeps it from posting.
    fs::write(root.join("post-empty"), "").unwrap();
    let _worker = start_worker(&root, &["--warm-ttl-seconds=1"]);
    run_helper(&root, APP, &reaction(ID1, THREAD));
    wait_for("the spare to start", || sends(&root).0.len() == 1);
    let spare = sends(&root).0[0].clone();
    wait_for("the spare to be discarded", || !alive(&spare));
    thread::sleep(Duration::from_millis(1_500));
    let (_, posted) = sends(&root);
    assert!(posted.is_empty(), "a discarded spare posted");
    fs::remove_file(root.join("post-empty")).unwrap();
    let body = root.join(format!("body-{spare}.txt"));
    assert!(fs::read(&body).map_or(true, |bytes| bytes.is_empty()));
    // The reply then takes the helper's own client run.
    let replied = run_helper(&root, APP, &send(ID2, THREAD, "late answer"));
    assert_eq!(replied["ok"], true, "{replied}");
    assert_eq!(sends(&root).1.len(), 1);
}

#[test]
fn a_worker_killed_while_it_holds_a_spare_leaves_no_post() {
    let root = workspace("killed");
    // As above: this client would post an empty body a second after its input ends, which the
    // worker's death closes, so only the parent-death signal keeps it from posting.
    fs::write(root.join("post-empty"), "").unwrap();
    let mut worker = start_worker(&root, &[]);
    run_helper(&root, APP, &reaction(ID1, THREAD));
    wait_for("the spare to start", || sends(&root).0.len() == 1);
    let spare = sends(&root).0[0].clone();
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    wait_for("the spare to die with its worker", || !alive(&spare));
    thread::sleep(Duration::from_millis(1_500));
    assert!(sends(&root).1.is_empty(), "the spare posted");
    fs::remove_file(root.join("post-empty")).unwrap();
    // The helper finds the socket dead and posts with its own run.
    let replied = run_helper(&root, APP, &send(ID2, THREAD, "after the worker"));
    assert_eq!(replied["ok"], true, "{replied}");
    assert_eq!(sends(&root).1.len(), 1);
}

#[test]
fn a_stopped_worker_delays_a_reply_by_the_ready_wait_and_never_posts_it_twice() {
    let root = workspace("stopped");
    let worker = start_worker(&root, &[]);
    run_helper(&root, APP, &reaction(ID1, THREAD));
    wait_for("the spare to start", || sends(&root).0.len() == 1);
    let spare = sends(&root).0[0].clone();
    let pid = worker.0.id().to_string();
    assert!(Command::new("kill")
        .args(["-STOP", &pid])
        .status()
        .unwrap()
        .success());
    // The stopped worker accepts the connection in the kernel but never answers, so the helper
    // gives up after its ready wait and posts with its own client run.
    let started = Instant::now();
    let replied = run_helper(&root, APP, &send(ID2, THREAD, "despite a stopped worker"));
    let waited = started.elapsed();
    assert_eq!(replied["ok"], true, "{replied}");
    assert!(waited >= Duration::from_secs(2), "{waited:?}");
    let (started_runs, posted) = sends(&root);
    assert_eq!(started_runs.len(), 2);
    assert_eq!(posted, [started_runs[1].clone()]);
    // Resumed, the worker reads the abandoned request, takes the spare, never gets a commit, and
    // discards the spare without posting.
    assert!(Command::new("kill")
        .args(["-CONT", &pid])
        .status()
        .unwrap()
        .success());
    wait_for("the abandoned spare to be discarded", || !alive(&spare));
    thread::sleep(Duration::from_millis(500));
    assert_eq!(sends(&root).1.len(), 1, "the reply was posted once");
}

#[test]
fn a_second_worker_for_the_same_socket_refuses_to_start() {
    let root = workspace("lock");
    let _worker = start_worker(&root, &[]);
    let mut arguments = common_arguments(&root, APP);
    arguments.push("--role=warm-worker".to_owned());
    let status = Command::new(env!("CARGO_BIN_EXE_agentctl-slack-outbound"))
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("FAKE_SLACK_DIR", &*root)
        .status()
        .expect("second worker runs");
    assert_eq!(status.code(), Some(1));
    // The first still serves.
    run_helper(&root, APP, &reaction(ID1, THREAD));
    wait_for("the spare to start", || sends(&root).0.len() == 1);
}

#[test]
fn a_worker_started_for_another_client_identity_holds_no_spare_for_the_helper() {
    // #383 slack-crewmate-outbound: the worker is its own unit with its own arguments. A worker
    // run as a Crewmate must not post a user-mode helper's reply without `--as`, nor the reverse,
    // so a helper whose identity or client arguments differ gets no spare and posts with its own
    // client run.
    let root = workspace("identity");
    let _worker = start_worker(&root, &["--client-identity=crewmate"]);
    run_helper(&root, APP, &reaction(ID1, THREAD));
    thread::sleep(Duration::from_millis(300));
    assert!(sends(&root).0.is_empty(), "no spare for another identity");
    let replied = run_helper(&root, APP, &send(ID2, THREAD, "own run"));
    assert_eq!(replied["ok"], true, "{replied}");
    let lines = log_lines(&root);
    let sent: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("slack.message send"))
        .collect();
    assert_eq!(sent.len(), 1, "{lines:?}");
    assert!(sent[0].contains(&format!("--as={APP}")), "{}", sent[0]);
}

//! Read native Codex goals without resuming or submitting input to a thread.
//!
//! A configured `codex app-server proxy` connects to the owning local daemon.
//! Versions that expose saved goals also support a separate
//! `codex app-server --stdio` process. Reading a saved goal does not wake its TUI.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Closed(bool),
    Error(String),
}

fn read_stream(
    mut stream: impl Read + Send + 'static,
    stdout: bool,
    sender: SyncSender<Event>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let event = match stream.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(Event::Closed(stdout));
                    return;
                }
                Ok(count) if stdout => Event::Stdout(buffer[..count].to_vec()),
                Ok(count) => Event::Stderr(buffer[..count].to_vec()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(Event::Error(format!(
                        "cannot read Codex goal transport: {error}"
                    )));
                    return;
                }
            };
            if sender.send(event).is_err() {
                return;
            }
        }
    })
}

struct Rpc {
    child: Child,
    input: ChildStdin,
    events: Option<Receiver<Event>>,
    readers: Vec<JoinHandle<()>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_closed: bool,
    stderr_closed: bool,
    deadline: Instant,
    timeout: Duration,
}

impl Drop for Rpc {
    fn drop(&mut self) {
        // Release readers blocked on the bounded channel before joining them.
        self.events.take();
        // The launched proxy/stdio process owns this group. An existing daemon
        // reached by a proxy belongs to another group and is never terminated.
        let _ = unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Rpc {
    fn new(command: &[String], timeout: Duration) -> Result<Self, String> {
        if timeout.is_zero() {
            return Err("native goal timeout must be positive".to_owned());
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| "native goal timeout is too large".to_owned())?;
        let (program, arguments) = command
            .split_first()
            .ok_or_else(|| "native goal command must not be empty".to_owned())?;
        if command
            .iter()
            .any(|argument| argument.is_empty() || argument.contains('\0'))
        {
            return Err("native goal command contains an invalid argument".to_owned());
        }
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| format!("cannot start Codex goal transport: {error}"))?;
        let input = child.stdin.take().expect("requested piped stdin");
        let stdout = child.stdout.take().expect("requested piped stdout");
        let stderr = child.stderr.take().expect("requested piped stderr");
        let (sender, receiver) = mpsc::sync_channel(16);
        let rpc = Self {
            child,
            input,
            events: Some(receiver),
            readers: vec![
                read_stream(stdout, true, sender.clone()),
                read_stream(stderr, false, sender),
            ],
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_closed: false,
            stderr_closed: false,
            deadline,
            timeout,
        };
        // A transport that never reads stdin must not turn writing into an
        // unbounded wait. fcntl changes only our owned pipe descriptor.
        let fd = rpc.input.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(format!(
                "cannot configure Codex goal transport: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(rpc)
    }

    fn remaining(&self) -> Result<Duration, String> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                format!(
                    "Codex goal RPC timed out after {} seconds",
                    self.timeout.as_secs_f64()
                )
            })
    }

    fn send(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("Codex goal request exceeds the message size limit".to_owned());
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let remaining = self.remaining()?;
            match self.input.write(&bytes[offset..]) {
                Ok(0) => return Err("Codex goal input pipe closed".to_owned()),
                Ok(count) => offset += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(error) => return Err(format!("cannot write Codex goal request: {error}")),
            }
        }
        Ok(())
    }

    fn receive(&mut self, request_id: i64) -> Result<Value, String> {
        loop {
            let remaining = self.remaining()?;
            if let Some(end) = self.stdout.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.stdout.drain(..=end).collect();
                let response: Value = serde_json::from_slice(&line)
                    .map_err(|error| format!("invalid Codex goal response: {error}"))?;
                let object = response
                    .as_object()
                    .ok_or_else(|| "Codex goal response is not an object".to_owned())?;
                if object.contains_key("method") {
                    if object.contains_key("id") {
                        return Err("Codex requested an interactive action during goal RPC; no approval was sent".to_owned());
                    }
                    continue;
                }
                if response.get("id").and_then(Value::as_i64) != Some(request_id) {
                    return Err("Codex returned an unexpected response ID".to_owned());
                }
                if let Some(error) = response.get("error") {
                    return Err(format!(
                        "Codex goal RPC refused: {}",
                        error.get("message").unwrap_or(error)
                    ));
                }
                let result = response
                    .get("result")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| "Codex goal result is not an object".to_owned())?;
                return Ok(result.clone());
            }
            if self.stdout_closed && self.stderr_closed {
                return Err(format!(
                    "Codex goal transport closed before replying: {}",
                    String::from_utf8_lossy(&self.stderr).trim()
                ));
            }
            let event = self
                .events
                .as_ref()
                .expect("live receiver")
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => format!(
                        "Codex goal RPC timed out after {} seconds",
                        self.timeout.as_secs_f64()
                    ),
                    mpsc::RecvTimeoutError::Disconnected => {
                        "Codex goal transport disconnected".to_owned()
                    }
                })?;
            match event {
                Event::Stdout(bytes) => {
                    self.stdout.extend(bytes);
                    if self.stdout.len() > MAX_MESSAGE_BYTES {
                        return Err("Codex goal response exceeds the message size limit".to_owned());
                    }
                }
                Event::Stderr(bytes) => {
                    self.stderr.extend(bytes);
                    if self.stderr.len() > 4096 {
                        self.stderr.drain(..self.stderr.len() - 4096);
                    }
                }
                Event::Closed(true) => self.stdout_closed = true,
                Event::Closed(false) => self.stderr_closed = true,
                Event::Error(error) => return Err(error),
            }
        }
    }
}

/// Return a native goal or `None` without starting or resuming the target thread.
///
/// `command` is a shell-free argv, normally `codex app-server proxy`, or an
/// explicitly selected stdio server. The entire handshake/read has one deadline;
/// errors and timeouts terminate only the launched transport process group.
pub fn get_goal(
    session_id: &str,
    command: &[String],
    timeout: Duration,
) -> Result<Option<Value>, String> {
    if session_id.is_empty() || session_id.contains('\0') {
        return Err("session_id must be a nonempty session identifier".to_owned());
    }
    let mut rpc = Rpc::new(command, timeout)?;
    rpc.send(json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "herdr-goal", "version": "1"},
            "capabilities": {"experimentalApi": true}
        }
    }))?;
    rpc.receive(1)?;
    rpc.send(json!({"method": "initialized", "params": {}}))?;
    rpc.send(json!({
        "id": 2,
        "method": "thread/goal/get",
        "params": {"threadId": session_id}
    }))?;
    let result = rpc.receive(2)?;
    let Some(goal) = result.get("goal").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    if !goal.is_object() {
        return Err("native Codex goal is not an object".to_owned());
    }
    if goal.get("threadId").and_then(Value::as_str) != Some(session_id) {
        return Err("native Codex goal belongs to another session".to_owned());
    }
    if goal.get("objective").and_then(Value::as_str).is_none() {
        return Err("native Codex goal objective is not a string".to_owned());
    }
    if !matches!(
        goal.get("status").and_then(Value::as_str),
        Some("active" | "paused" | "blocked" | "usageLimited" | "budgetLimited" | "complete")
    ) {
        return Err("native Codex goal has an unknown status".to_owned());
    }
    for key in ["tokensUsed", "timeUsedSeconds", "createdAt", "updatedAt"] {
        if goal.get(key).and_then(Value::as_i64).is_none() {
            return Err(format!("native Codex goal {key} is not an integer"));
        }
    }
    if goal
        .get("tokenBudget")
        .is_some_and(|value| !value.is_null() && value.as_i64().is_none())
    {
        return Err("native Codex goal tokenBudget is not an integer".to_owned());
    }
    Ok(Some(goal.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    const SERVER: &str = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$1/calls.jsonl"
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"method":"initialized"'*) ;;
    *'"method":"thread/goal/get"'*)
      case "$2" in
        hang) sleep 60 & printf '%s' "$!" > "$1/child.tmp" && mv "$1/child.tmp" "$1/child.pid"; wait ;;
        error) printf '%s\n' '{"id":2,"error":{"code":-32601,"message":"unknown goal method"}}'; continue ;;
        closed) printf '%s\n' 'fixture failure' >&2; exit 4 ;;
        invalid) printf '%s\n' 'invalid-json'; continue ;;
        interactive) printf '%s\n' '{"id":98,"method":"requestApproval","params":{}}'; continue ;;
        absent) printf '%s\n' '{"id":2,"result":{"goal":null}}'; continue ;;
        wrong-session) printf '%s\n' '{"id":2,"result":{"goal":{"threadId":"other"}}}'; continue ;;
        wrong-id) printf '%s\n' '{"id":99,"result":{}}'; continue ;;
        stderr) i=0; while [ "$i" -lt 20000 ]; do printf '%s\n' 'launcher-log' >&2; i=$((i+1)); done ;;
      esac
      printf '%s\n' '{"method":"thread/goal/updated","params":{}}'
      printf '%s' '{"id":2,"result":'
      if [ "$2" = fragmented ]; then sleep 0.01; fi
      printf '%s\n' '{"goal":{"threadId":"thread-fixture","objective":"Observe progress","status":"active","tokenBudget":null,"tokensUsed":10,"timeUsedSeconds":2,"createdAt":100,"updatedAt":101}}}'
      ;;
  esac
done
"#;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "herdr-goal-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn command(&self, mode: &str) -> Vec<String> {
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                SERVER.to_owned(),
                "goal-fixture".to_owned(),
                self.0.to_string_lossy().into_owned(),
                mode.to_owned(),
            ]
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_performs_no_resume_or_turn_and_drains_both_streams() {
        for mode in ["normal", "fragmented", "stderr"] {
            let fixture = Fixture::new();
            let goal = get_goal(
                "thread-fixture",
                &fixture.command(mode),
                Duration::from_secs(5),
            )
            .unwrap()
            .unwrap();
            assert_eq!(goal["tokensUsed"], 10);
            let calls: Vec<Value> = fs::read_to_string(fixture.0.join("calls.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                calls
                    .iter()
                    .map(|call| call["method"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                vec!["initialize", "initialized", "thread/goal/get"]
            );
            assert_eq!(calls[2]["params"], json!({"threadId":"thread-fixture"}));
        }
    }

    #[test]
    fn absent_goal_and_protocol_failures_remain_distinct() {
        let fixture = Fixture::new();
        assert!(get_goal(
            "thread-fixture",
            &fixture.command("absent"),
            Duration::from_secs(5)
        )
        .unwrap()
        .is_none());
        for (mode, message) in [
            ("error", "unknown goal method"),
            ("invalid", "invalid Codex goal response"),
            ("closed", "fixture failure"),
            ("interactive", "no approval was sent"),
            ("wrong-session", "belongs to another session"),
            ("wrong-id", "unexpected response ID"),
        ] {
            let error = get_goal(
                "thread-fixture",
                &fixture.command(mode),
                Duration::from_secs(5),
            )
            .unwrap_err();
            assert!(error.contains(message), "{mode}: {error}");
        }
    }

    #[test]
    fn timeout_kills_transport_descendants() {
        // The deadline starts before the transport shell runs, so a loaded host can expire it
        // before the fixture publishes its descendant. Such an attempt never exercised a
        // descendant; retry it with a longer deadline rather than read an unpublished marker.
        for timeout in [300, 1_000, 3_000, 10_000].map(Duration::from_millis) {
            let fixture = Fixture::new();
            let start = Instant::now();
            let error = get_goal("thread-fixture", &fixture.command("hang"), timeout).unwrap_err();
            assert!(error.contains("timed out"), "{error}");
            assert!(start.elapsed() < timeout + Duration::from_secs(5));
            let Ok(pid) = fs::read_to_string(fixture.0.join("child.pid")) else {
                continue;
            };
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Err(_) => return,
                    Ok(stat)
                        if stat
                            .rsplit_once(") ")
                            .is_some_and(|(_, tail)| tail.starts_with('Z')) =>
                    {
                        return
                    }
                    Ok(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
            panic!("timed-out transport child {pid} is still running");
        }
        panic!("the hang fixture never published its descendant before any deadline");
    }
}

//! `agentctl console NAME` end to end (#670 agentctl-console): a real
//! console under a pty against a headless agent with a fake stream-json
//! `claude` on PATH. A typed line must reach the harness byte for byte
//! as a plain user turn (where `agentctl send` of the same text carries
//! a sender header), a bracketed paste must arrive as one message, an
//! approval-shaped line must never reach the harness, and Ctrl-C must
//! leave the console without touching the agent. Skips (passes) when
//! python3 is not installed. Linux only.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The turnover fake's protocol, minus turnover: acknowledge, start,
/// answer with an echo, complete; record every input's text.
const FAKE_CLAUDE: &str = r##"#!/usr/bin/env python3
import json, os, re, sys, threading, queue
args = sys.argv[1:]
session = args[args.index("--session-id") + 1] if "--session-id" in args else args[args.index("--resume") + 1]
config = os.environ["CLAUDE_CONFIG_DIR"]
project = re.sub(r"[^A-Za-z0-9]", "-", os.getcwd())
transcript = os.path.join(config, "projects", project, session + ".jsonl")
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
while True:
    message = inputs.get()
    if message is None:
        break
    if message.get("type") == "control_request":
        out({"type": "control_response", "response": {"subtype": "success",
             "request_id": message["request_id"], "response": {"still_queued": []}}})
        continue
    uuid = message["uuid"]
    text = message["message"]["content"]
    with open(os.environ["FAKE_INPUTS"], "a") as log:
        log.write(json.dumps({"session": session, "text": text}) + "\n")
    life(uuid, "queued")
    life(uuid, "started")
    os.makedirs(os.path.dirname(transcript), exist_ok=True)
    with open(transcript, "a") as handle:
        handle.write(json.dumps({"type": "user", "uuid": uuid}) + "\n")
    out({"type": "system", "subtype": "init", "claude_code_version": "9.9.9", "model": "fake",
         "permissionMode": "dontAsk", "session_id": session, "tools": ["Bash"]})
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo tool-marker"}}]}})
    out({"type": "user", "parent_tool_use_id": None, "message": {"content": [
        {"type": "tool_result", "tool_use_id": "t1", "content": "tool ran"}]}})
    out({"type": "assistant", "parent_tool_use_id": None, "message": {"content": [
        {"type": "text", "text": "echo: " + text}]}})
    out({"type": "result", "subtype": "success", "duration_ms": 5,
         "usage": {"input_tokens": 1, "output_tokens": 2}, "permission_denials": []})
    life(uuid, "completed")
"##;

/// Double-fork so the console's parentage reaches init, never a harness
/// an ancestor scan could find (the suite itself may run inside one).
const LAUNCHER: &str = r##"
import fcntl, os, sys, termios
pidfile, agentctl, registry = sys.argv[1], sys.argv[2], sys.argv[3]
if os.fork() > 0:
    os._exit(0)
os.setsid()
fcntl.ioctl(0, termios.TIOCSCTTY, 0)
with open(pidfile, "w") as handle:
    handle.write(str(os.getpid()))
os.execvpe(agentctl, [agentctl, "--registry", registry, "console", "worker"], os.environ)
"##;

fn executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

struct Fixture {
    root: PathBuf,
    registry: PathBuf,
    work: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    inputs: PathBuf,
}

impl Fixture {
    fn new() -> Option<Self> {
        executable("python3")?;
        let root = std::env::temp_dir().join(format!(
            "agentctl-console-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_nanos())
                .unwrap_or(0)
        ));
        let work = root.join("work");
        let bin = root.join("bin");
        let config = root.join("claude-config");
        for directory in [&root, &work, &bin, &config] {
            fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(directory)
                .unwrap();
        }
        let fake = bin.join("claude");
        fs::write(&fake, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        Some(Self {
            registry: root.join("registry"),
            inputs: root.join("inputs.log"),
            root,
            work,
            bin,
            config,
        })
    }

    fn agentctl(&self, args: &[&str]) -> std::process::Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .arg("--registry")
            .arg(&self.registry)
            .args(args)
            .env("PATH", path)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("FAKE_INPUTS", &self.inputs)
            .env_remove("AGENTCTL_AGENT")
            .env_remove("HERDR_WORKSPACE_ID")
            .output()
            .expect("run agentctl")
    }

    fn texts(&self) -> Vec<String> {
        fs::read_to_string(&self.inputs)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                value["text"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    fn wait_for(&self, what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.agentctl(&["stop", "worker"]);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One console under a pty: the master side, the accumulated output,
/// and the console's own pid (from its pidfile, never a name search).
struct Console {
    master: fs::File,
    output: String,
    pid: i32,
}

impl Console {
    fn pump(&mut self) {
        let mut buffer = [0_u8; 8192];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    self.output
                        .push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
                Err(_) => break,
            }
        }
    }

    fn wait_for(&mut self, what: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.output.contains(needle) {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} ({needle:?}) in console output:\n{}",
                self.output
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn type_line(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
    }

    fn alive(&self) -> bool {
        Path::new(&format!("/proc/{}", self.pid)).exists()
    }

    /// The final screen a terminal would show: interpret CR, LF and
    /// the console's clear-line / cursor-right sequences, so text that
    /// was written and then erased does not count as shown.
    fn screen_lines(&self) -> Vec<String> {
        let mut lines: Vec<Vec<char>> = vec![Vec::new()];
        let (mut row, mut col) = (0_usize, 0_usize);
        let bytes = self.output.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                0x1b if index + 1 < bytes.len() && bytes[index + 1] == b'[' => {
                    let mut end = index + 2;
                    while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                        end += 1;
                    }
                    if end >= bytes.len() {
                        break;
                    }
                    let params = &self.output[index + 2..end];
                    match bytes[end] {
                        b'K' => {
                            if row < lines.len() {
                                lines[row].truncate(col);
                            }
                        }
                        b'C' => {
                            col += params.parse::<usize>().unwrap_or(1);
                        }
                        _ => {}
                    }
                    index = end + 1;
                }
                0x1b => {
                    index += 2;
                }
                b'\r' => {
                    col = 0;
                    index += 1;
                }
                b'\n' => {
                    row += 1;
                    col = 0;
                    if row >= lines.len() {
                        lines.push(Vec::new());
                    }
                    index += 1;
                }
                _ => {
                    let text = &self.output[index..];
                    let Some(ch) = text.chars().next() else { break };
                    if !ch.is_control() {
                        if row >= lines.len() {
                            lines.push(Vec::new());
                        }
                        if col >= lines[row].len() {
                            lines[row].resize(col, ' ');
                            lines[row].push(ch);
                        } else {
                            lines[row][col] = ch;
                        }
                        col += 1;
                    }
                    index += ch.len_utf8();
                }
            }
        }
        lines
            .iter()
            .map(|line| line.iter().collect::<String>())
            .collect()
    }

    fn screen_contains(&self, needle: &str) -> bool {
        self.screen_lines().iter().any(|line| line.contains(needle))
    }

    fn wait_for_screen(&mut self, what: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.screen_contains(needle) {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} ({needle:?}) on the console screen:\n{}\n--- raw ---\n{}",
                self.screen_lines().join("\n"),
                self.output
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Launch `agentctl console worker` orphaned onto a fresh pty.
fn launch_console(fixture: &Fixture) -> Console {
    let (mut master_fd, mut slave_fd) = (-1, -1);
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "openpty failed");
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &size) };
    let pidfile = fixture.root.join("console.pid");
    let launcher = fixture.root.join("launch_console.py");
    fs::write(&launcher, LAUNCHER).unwrap();
    let path = format!(
        "{}:{}",
        fixture.bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let slave_in = unsafe { fs::File::from_raw_fd(slave_fd) };
    let slave_out = unsafe { fs::File::from_raw_fd(libc::dup(slave_fd)) };
    let slave_err = unsafe { fs::File::from_raw_fd(libc::dup(slave_fd)) };
    let mut launched = Command::new("python3")
        .arg(&launcher)
        .arg(&pidfile)
        .arg(env!("CARGO_BIN_EXE_agentctl"))
        .arg(fixture.registry.as_os_str())
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", &fixture.config)
        .env("FAKE_INPUTS", &fixture.inputs)
        .env("USER", "alice")
        .env_remove("AGENTCTL_AGENT")
        .env_remove("HERDR_WORKSPACE_ID")
        .stdin(Stdio::from(slave_in))
        .stdout(Stdio::from(slave_out))
        .stderr(Stdio::from(slave_err))
        .spawn()
        .expect("launch console");
    let _ = launched.wait();
    // SAFETY: fcntl only sets the master nonblocking.
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL);
        libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let master = unsafe { fs::File::from_raw_fd(master_fd) };
    fixture.wait_for("console pid", || pidfile.exists());
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    Console {
        master,
        output: String::new(),
        pid,
    }
}

#[test]
fn console_shows_a_follower_error_instead_of_silence() {
    let Some(fixture) = Fixture::new() else {
        eprintln!("python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();
    let started = fixture.agentctl(&[
        "start",
        "worker",
        "--mode",
        "headless",
        "--harness",
        "claude",
        "--effort",
        "high",
        "--cwd",
        &work,
    ]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    fixture.wait_for("control socket", || {
        fixture.registry.join("agents/worker/control.sock").exists()
    });
    // Find the event log directory wherever the registry layout puts it.
    fn find_events(dir: &Path) -> Option<PathBuf> {
        if dir.file_name().is_some_and(|name| name == "events") && dir.is_dir() {
            return Some(dir.to_path_buf());
        }
        for entry in fs::read_dir(dir).ok()? {
            let path = entry.ok()?.path();
            if path.is_dir() {
                if let Some(found) = find_events(&path) {
                    return Some(found);
                }
            }
        }
        None
    }
    let events = find_events(&fixture.registry).expect("event log directory");
    // Make the follower fail while the agent record stays intact: the
    // event log path is a file, not a directory, so reading its
    // segments errors at follower start.
    fs::remove_dir_all(&events).unwrap();
    fs::write(&events, b"not a directory").unwrap();
    let mut console = launch_console(&fixture);
    console.wait_for_screen("follower error shown", "console follower failed");
    console.wait_for_screen(
        "follower death announced",
        "transcript is no longer updating",
    );
    console.type_line(b"/quit\r");
    fixture.wait_for("console exit", || !console.alive());
}

#[test]
fn console_delivers_typed_lines_as_plain_turns() {
    let Some(fixture) = Fixture::new() else {
        eprintln!("python3 not installed; skipping");
        return;
    };
    let work = fixture.work.display().to_string();
    let started = fixture.agentctl(&[
        "start",
        "worker",
        "--mode",
        "headless",
        "--harness",
        "claude",
        "--effort",
        "high",
        "--cwd",
        &work,
    ]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    fixture.wait_for("control socket", || {
        fixture.registry.join("agents/worker/control.sock").exists()
    });

    let mut console = launch_console(&fixture);
    console.wait_for_screen("banner on screen", "agentctl console worker");

    // A typed line arrives byte for byte: no sender header anywhere.
    console.type_line(b"run `date`\r");
    console.wait_for_screen("queue ack on screen", "queued ");
    console.wait_for_screen("queue position on screen", "position 1");
    fixture.wait_for("harness input", || fixture.texts().len() == 1);
    assert_eq!(fixture.texts(), ["run `date`"]);
    console.wait_for("console origin in the log", "console:alice");
    // The turn's assistant text and tool line must be on the final
    // screen (written and not erased by the input-line redraw), and
    // must survive a later redraw while typing a draft.
    console.wait_for_screen("assistant text on screen", "echo: run");
    console.wait_for_screen("tool line on screen", "echo tool-marker");
    console.type_line(b"draft");
    console.wait_for_screen("draft on the input line", "draft");
    assert!(
        console.screen_contains("echo: run"),
        "assistant text erased by redraw:\n{}",
        console.screen_lines().join("\n")
    );
    assert!(
        console.screen_contains("echo tool-marker"),
        "tool line erased by redraw:\n{}",
        console.screen_lines().join("\n")
    );
    console.type_line(&[0x15]);

    // /help and an unknown /command are handled locally, never queued.
    console.type_line(b"/help\r");
    console.wait_for_screen("/help lists /interrupt", "/interrupt");
    console.wait_for_screen("/help lists /quit", "/quit");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fixture.texts().len(), 1, "/help reached the harness");
    console.type_line(b"/bogus\r");
    console.wait_for_screen("unknown command refused", "unknown command");
    console.wait_for_screen("unknown command hints /help", "/help");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fixture.texts().len(), 1, "/bogus reached the harness");

    // A bracketed paste is one message, newlines kept.
    console.type_line(b"\x1b[200~first line\nsecond line\x1b[201~");
    console.wait_for("paste summary", "[paste: 2 lines");
    console.type_line(b"\r");
    fixture.wait_for("paste delivered", || fixture.texts().len() == 2);
    assert_eq!(fixture.texts()[1], "first line\nsecond line");

    // An approval-shaped line is answered, never delivered.
    console.type_line(b"approve a99\r");
    console.wait_for("approval refusal", "approval not recorded");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        fixture.texts().len(),
        2,
        "approval line reached the harness"
    );

    // The same text through `agentctl send` carries its sender header.
    let sent = fixture.agentctl(&["send", "worker", "run `date`"]);
    assert!(sent.status.success(), "{sent:?}");
    fixture.wait_for("send delivered", || fixture.texts().len() == 3);
    assert!(
        fixture.texts()[2].starts_with("[from "),
        "send must carry a header: {:?}",
        fixture.texts()[2]
    );

    // `/interrupt` interrupts through the control socket; it is never
    // delivered as a message.
    console.type_line(b"/interrupt\r");
    console.wait_for("interrupt ack", "interrupt sent");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        fixture.texts().len(),
        3,
        "/interrupt reached the harness as a message"
    );

    // A single empty-line Ctrl-C only hints; two inside the window
    // interrupt; a stray Ctrl-C never leaves the console.
    console.type_line(&[0x03]);
    console.wait_for("ctrl-c hint", "press Ctrl-C again to interrupt");
    assert!(
        console.alive(),
        "a single Ctrl-C must not leave the console"
    );
    console.type_line(&[0x03]);
    console.wait_for("double ctrl-c interrupt", "interrupt sent");
    assert!(console.alive(), "Ctrl-C must never leave the console");

    // /quit leaves, prints the disconnected notice, and the agent is
    // untouched and stoppable.
    console.type_line(b"/quit\r");
    fixture.wait_for("console exit", || !console.alive());
    console.wait_for(
        "disconnected notice",
        "Disconnected from worker; the agent is still running",
    );
    let status = fixture.agentctl(&["status", "worker"]);
    assert!(status.status.success(), "{status:?}");
}

//! `agentctl console NAME`: the owner's input line for a local headless
//! agent (#670 agentctl-console).
//!
//! The console follows the agent's event log with the `tail --follow`
//! rendering and adds a raw-mode input line at the bottom. A submitted
//! line is not typed into any pane and is not queued by the console
//! itself: it goes to the wrapper as one `console.input` control
//! request, and the wrapper — after proving the peer — queues it as a
//! plain user turn, byte for byte, with its origin logged. Approval
//! answers (`approve a7`, `deny a7 too broad`) go through the existing
//! approval path and are never delivered as messages. `/interrupt`, or
//! Ctrl-C twice on an empty line inside the double-press window, asks
//! the wrapper to interrupt the running turn; a single Ctrl-C only
//! hints and never leaves. Ctrl-D, `/quit`, or a signal leaves the
//! console, every exit prints the disconnected notice, and leaving
//! never touches the agent.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::headless::approval::parse_reply;
use crate::headless::control as hcontrol;
use crate::headless::tail::{self, TailOptions};
use crate::headless::wrapper::CONSOLE_INPUT_MAX_BYTES;
use crate::subagents::{ManagedAgents, ManagedApi};

/// Longest a control request may wait for the wrapper's answer.
const CONTROL_WAIT: Duration = Duration::from_secs(20);

/// Shared console output: the event follower and the input line draw on
/// the same stdout, one locked section at a time.
struct Shared {
    out: Mutex<io::Stdout>,
    editor: Mutex<LineEditor>,
    name: String,
}

impl Shared {
    fn redraw(&self, out: &mut io::Stdout) -> io::Result<()> {
        let editor = self.editor.lock().expect("editor lock poisoned");
        let (display, cursor) = editor.display();
        write!(out, "\r\x1b[K{} › {display}", self.name)?;
        let prompt = self.name.chars().count() + 3 + cursor;
        write!(out, "\r\x1b[{prompt}C")?;
        out.flush()
    }

    /// Print one finished line above the input line, then redraw it.
    fn print_line(&self, text: &str) {
        let mut out = self.out.lock().expect("stdout lock poisoned");
        let _ = write!(out, "\r\x1b[K{text}\r\n");
        let _ = self.redraw(&mut out);
    }
}

/// The follower's writer: each write replaces the input row, prints the
/// rendered events (raw mode needs CRLF), and redraws the input line.
struct ConsoleWriter {
    shared: Arc<Shared>,
}

impl Write for ConsoleWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut out = self.shared.out.lock().expect("stdout lock poisoned");
        out.write_all(b"\r\x1b[K")?;
        let mut start = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r') {
                out.write_all(&bytes[start..index])?;
                out.write_all(b"\r\n")?;
                start = index + 1;
            }
        }
        out.write_all(&bytes[start..])?;
        self.shared.redraw(&mut out)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.shared
            .out
            .lock()
            .expect("stdout lock poisoned")
            .flush()
    }
}

/// What one key or byte did to the line.
enum Action {
    /// Nothing visible changed.
    None,
    /// Redraw the input line.
    Changed,
    /// Enter: submit the buffer.
    Submit,
    /// Ctrl-C twice on an empty line within [`DOUBLE_CTRL_C`]:
    /// interrupt the running turn.
    Interrupt,
    /// First empty-line Ctrl-C: show the interrupt hint. A stray
    /// Ctrl-C never leaves the console.
    Hint,
    /// Leave the console (Ctrl-D on an empty line, or `/quit`).
    Quit,
}

/// How soon a second empty-line Ctrl-C must follow the first to
/// interrupt the turn (thread 316).
const DOUBLE_CTRL_C: Duration = Duration::from_secs(2);

/// The input line: text as chars with a char cursor, an incremental
/// escape-sequence parser, and bracketed-paste collection.
#[derive(Default)]
struct LineEditor {
    text: Vec<char>,
    cursor: usize,
    escape: Escape,
    paste: Vec<u8>,
    utf8: Vec<u8>,
    ctrl_c_at: Option<std::time::Instant>,
}

#[derive(Default, PartialEq)]
enum Escape {
    #[default]
    Ground,
    Esc,
    Bracket(Vec<u8>),
    Paste,
}

impl LineEditor {
    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn take_text(&mut self) -> String {
        let text: String = self.text.iter().collect();
        self.clear();
        text
    }

    fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += 1;
    }

    fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert(ch);
        }
    }

    /// The one-row view of the buffer and the cursor column inside it:
    /// a buffer holding newlines (a paste) shows as a summary row.
    fn display(&self) -> (String, usize) {
        if self.text.contains(&'\n') {
            let text: String = self.text.iter().collect();
            let lines = text.matches('\n').count() + 1;
            let shown = format!("[paste: {lines} lines, {} bytes]", text.len());
            let cursor = shown.chars().count();
            (shown, cursor)
        } else {
            (self.text.iter().collect(), self.cursor)
        }
    }

    fn feed(&mut self, byte: u8) -> Action {
        match self.escape {
            Escape::Paste => return self.feed_paste(byte),
            Escape::Esc => {
                self.escape = if byte == b'[' {
                    Escape::Bracket(Vec::new())
                } else {
                    Escape::Ground
                };
                return Action::None;
            }
            Escape::Bracket(_) => return self.feed_bracket(byte),
            Escape::Ground => {}
        }
        if byte != 0x03 {
            self.ctrl_c_at = None;
        }
        match byte {
            0x03 => {
                // Ctrl-C: clear a non-empty buffer. On an empty buffer
                // the first press only hints; a second inside the
                // double-press window interrupts the turn. Ctrl-C
                // never leaves the console.
                if self.text.is_empty() {
                    let now = std::time::Instant::now();
                    if self
                        .ctrl_c_at
                        .is_some_and(|at| now.duration_since(at) <= DOUBLE_CTRL_C)
                    {
                        self.ctrl_c_at = None;
                        Action::Interrupt
                    } else {
                        self.ctrl_c_at = Some(now);
                        Action::Hint
                    }
                } else {
                    self.ctrl_c_at = None;
                    self.clear();
                    Action::Changed
                }
            }
            0x04 => {
                // Ctrl-D: delete under the cursor, else leave when empty.
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                    Action::Changed
                } else if self.text.is_empty() {
                    Action::Quit
                } else {
                    Action::None
                }
            }
            b'\r' => Action::Submit,
            b'\n' => {
                self.insert('\n');
                Action::Changed
            }
            0x7f | 0x08 => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.cursor);
                    Action::Changed
                } else {
                    Action::None
                }
            }
            0x01 => {
                self.cursor = 0;
                Action::Changed
            }
            0x05 => {
                self.cursor = self.text.len();
                Action::Changed
            }
            0x0b => {
                self.text.truncate(self.cursor);
                Action::Changed
            }
            0x15 => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
                Action::Changed
            }
            0x17 => {
                // Ctrl-W: drop the word before the cursor.
                let mut start = self.cursor;
                while start > 0 && self.text[start - 1] == ' ' {
                    start -= 1;
                }
                while start > 0 && self.text[start - 1] != ' ' {
                    start -= 1;
                }
                self.text.drain(start..self.cursor);
                self.cursor = start;
                Action::Changed
            }
            0x1b => {
                self.escape = Escape::Esc;
                Action::None
            }
            0x00..=0x1f => Action::None,
            _ => {
                self.utf8.push(byte);
                match std::str::from_utf8(&self.utf8) {
                    Ok(text) => {
                        let text = text.to_owned();
                        self.utf8.clear();
                        self.insert_str(&text);
                        Action::Changed
                    }
                    Err(error) if error.error_len().is_some() => {
                        self.utf8.clear();
                        Action::None
                    }
                    Err(_) => Action::None,
                }
            }
        }
    }

    fn feed_bracket(&mut self, byte: u8) -> Action {
        let Escape::Bracket(mut sequence) = std::mem::take(&mut self.escape) else {
            unreachable!("feed_bracket outside a bracket sequence");
        };
        if !(0x40..=0x7e).contains(&byte) && sequence.len() < 8 {
            sequence.push(byte);
            self.escape = Escape::Bracket(sequence);
            return Action::None;
        }
        self.escape = Escape::Ground;
        let sequence = String::from_utf8_lossy(&sequence).into_owned();
        match (sequence.as_str(), byte) {
            ("200", b'~') => {
                self.escape = Escape::Paste;
                self.paste.clear();
                Action::None
            }
            ("", b'D') => {
                self.cursor = self.cursor.saturating_sub(1);
                Action::Changed
            }
            ("", b'C') => {
                self.cursor = (self.cursor + 1).min(self.text.len());
                Action::Changed
            }
            ("", b'H') | ("1", b'~') | ("7", b'~') => {
                self.cursor = 0;
                Action::Changed
            }
            ("", b'F') | ("4", b'~') | ("8", b'~') => {
                self.cursor = self.text.len();
                Action::Changed
            }
            ("3", b'~') => {
                if self.cursor < self.text.len() {
                    self.text.remove(self.cursor);
                }
                Action::Changed
            }
            _ => Action::None,
        }
    }

    fn feed_paste(&mut self, byte: u8) -> Action {
        self.paste.push(byte);
        const END: &[u8] = b"\x1b[201~";
        if self.paste.ends_with(END) {
            self.paste.truncate(self.paste.len() - END.len());
            let raw = String::from_utf8_lossy(&self.paste).into_owned();
            self.paste.clear();
            self.escape = Escape::Ground;
            let text = raw.replace("\r\n", "\n").replace('\r', "\n");
            self.insert_str(&text);
            return Action::Changed;
        }
        Action::None
    }
}

/// Raw mode with bracketed paste, restored on drop (exit, panic, or a
/// caught signal's unwind all pass through here).
struct RawTerminal {
    original: libc::termios,
}

impl RawTerminal {
    fn enter() -> io::Result<Self> {
        // SAFETY: tcgetattr writes only into the termios it is given.
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(0, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        // SAFETY: cfmakeraw rewrites only the termios it is given.
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = io::stdout().lock();
        out.write_all(b"\x1b[?2004h")?;
        out.flush()?;
        Ok(Self { original })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let mut out = io::stdout().lock();
        let _ = out.write_all(b"\x1b[?2004l\r\n");
        let _ = out.flush();
        // SAFETY: restoring the saved termios for descriptor 0.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.original) };
    }
}

/// The owner-at-a-terminal proof the console requires before it starts
/// (#670): both standard streams are terminals, standard input is this
/// process's controlling terminal in its foreground group,
/// `AGENTCTL_AGENT` is unset, and no coding-agent harness is an
/// ancestor. Returns the `(user, tty)` the input is logged under.
fn authenticate() -> Result<(String, String), String> {
    // SAFETY: isatty and the process-ID calls only inspect.
    unsafe {
        if libc::isatty(0) != 1 {
            return Err("console needs a terminal on standard input".to_owned());
        }
        if libc::isatty(1) != 1 {
            return Err("console needs a terminal on standard output".to_owned());
        }
        if libc::tcgetsid(0) != libc::getsid(0) || libc::tcgetpgrp(0) != libc::getpgrp() {
            return Err(
                "console must run in the foreground of its controlling terminal".to_owned(),
            );
        }
    }
    if std::env::var("AGENTCTL_AGENT").is_ok_and(|value| !value.is_empty()) {
        return Err(
            "console answers for the owner, not an agent (AGENTCTL_AGENT is set)".to_owned(),
        );
    }
    if let Some((program, pid)) =
        crate::sender_header::harness_ancestor(Path::new("/proc"), std::process::id())
    {
        return Err(format!(
            "console refuses to run inside a harness ({program} pid {pid}); a lane cannot type as the owner"
        ));
    }
    let user = std::env::var("USER")
        .ok()
        .filter(|user| {
            !user.is_empty() && !user.chars().any(|c| c.is_whitespace() || c.is_control())
        })
        // SAFETY: getuid cannot fail.
        .unwrap_or_else(|| format!("uid{}", unsafe { libc::getuid() }));
    // SAFETY: ttyname returns a static buffer or null for descriptor 0.
    let tty = unsafe {
        let name = libc::ttyname(0);
        if name.is_null() {
            return Err("cannot name the terminal on standard input".to_owned());
        }
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    };
    Ok((user, tty))
}

/// Act on one submitted line: an approval answer when the line reads as
/// one (never delivered as a message), otherwise one `console.input`
/// request. The buffer is kept when nothing was queued, so a line is
/// never lost to a stopped wrapper.
/// Returns true when the line was `/quit`: leave the console.
fn submit<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    socket: &Path,
    name: &str,
    user: &str,
    tty: &str,
    shared: &Arc<Shared>,
) -> bool {
    let line = {
        let mut editor = shared.editor.lock().expect("editor lock poisoned");
        editor.take_text()
    };
    let line = line.trim_end().to_owned();
    if line.is_empty() {
        return false;
    }
    if line == "/quit" {
        return true;
    }
    if line == "/interrupt" {
        send_interrupt(socket, shared);
        return false;
    }
    if let Some(reply) = parse_reply(&line) {
        let Some(id) = reply.id else {
            shared.print_line("name the approval: approve aN or deny aN [reason]");
            return false;
        };
        match manager.headless_answer(name, Some(&id), reply.decision, reply.reason.as_deref()) {
            Ok(_) => shared.print_line(&format!("{} {id} recorded", reply.decision.as_str())),
            Err(error) => shared.print_line(&format!("approval not recorded: {error}")),
        }
        return false;
    }
    if line.len() > CONSOLE_INPUT_MAX_BYTES {
        restore_buffer(shared, &line);
        shared.print_line(&format!(
            "message too large (at most {CONSOLE_INPUT_MAX_BYTES} bytes); kept"
        ));
        return false;
    }
    let body = json!({"op": "console.input", "text": line, "user": user, "tty": tty});
    match hcontrol::request(socket, &body, CONTROL_WAIT) {
        Ok(answer) if answer["ok"] == json!(true) => {
            shared.print_line(&format!(
                "queued {}",
                answer["queue_id"].as_str().unwrap_or("?")
            ));
        }
        Ok(answer) => {
            restore_buffer(shared, &line);
            shared.print_line(&format!(
                "not queued: {}",
                answer["error"].as_str().unwrap_or("the wrapper refused")
            ));
        }
        Err(hcontrol::ClientError::NotRunning(_)) => {
            restore_buffer(shared, &line);
            shared.print_line("the headless wrapper is not running; message kept");
        }
        Err(hcontrol::ClientError::Unreachable(detail)) => {
            restore_buffer(shared, &line);
            shared.print_line(&format!("not queued: {detail}"));
        }
    }
    false
}

/// Ask the wrapper to interrupt the running turn (the `/interrupt` line
/// and Ctrl-C on an empty line). Interrupting never leaves the console.
fn send_interrupt(socket: &Path, shared: &Arc<Shared>) {
    match hcontrol::request(socket, &json!({"op": "interrupt"}), CONTROL_WAIT) {
        Ok(answer) if answer["ok"] == json!(true) => {
            shared.print_line("interrupt sent");
        }
        Ok(answer) => shared.print_line(&format!(
            "not interrupted: {}",
            answer["error"].as_str().unwrap_or("the wrapper refused")
        )),
        Err(hcontrol::ClientError::NotRunning(_)) => {
            shared.print_line("the headless wrapper is not running; nothing to interrupt");
        }
        Err(hcontrol::ClientError::Unreachable(detail)) => {
            shared.print_line(&format!("not interrupted: {detail}"));
        }
    }
}

fn restore_buffer(shared: &Arc<Shared>, line: &str) {
    let mut editor = shared.editor.lock().expect("editor lock poisoned");
    editor.clear();
    editor.insert_str(line);
}

/// Run the console until the owner leaves it; `0` on a clean exit.
pub fn run<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    registry: &Path,
    name: &str,
    mut options: TailOptions,
) -> Result<i32, String> {
    let (user, tty) = authenticate()?;
    if !manager
        .is_headless(name)
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "console is for local headless agents; {name:?} is not one (a pane agent has a terminal: agentctl attach)"
        ));
    }
    let directory =
        agent_queue::registry::directory(registry, name).map_err(|error| error.to_string())?;
    let events = crate::headless::events::directory(&directory);
    if !events.is_dir() {
        eprintln!("agentctl console: waiting for {name}'s first event (no event log yet)");
    }
    let socket = hcontrol::socket_path(&directory);
    let quit = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(libc::SIGTERM, quit.clone());
    let _ = signal_hook::flag::register(libc::SIGHUP, quit.clone());
    let _raw = RawTerminal::enter().map_err(|error| error.to_string())?;
    let shared = Arc::new(Shared {
        out: Mutex::new(io::stdout()),
        editor: Mutex::new(LineEditor::default()),
        name: name.to_owned(),
    });
    options.follow = true;
    options.console = true;
    let follower_shared = shared.clone();
    let follower_quit = quit.clone();
    let follower = std::thread::Builder::new()
        .name("console-follow".to_owned())
        .spawn(move || {
            let mut writer = ConsoleWriter {
                shared: follower_shared,
            };
            let _ = tail::run(&events, &options, &mut writer, &|| {
                follower_quit.load(Ordering::Relaxed)
            });
        })
        .map_err(|error| error.to_string())?;
    shared.print_line(&format!(
        "agentctl console {name}: Enter sends a plain turn; /interrupt or Ctrl-C twice interrupts the turn; Ctrl-D or /quit leaves; approve aN / deny aN [reason] answers an approval"
    ));
    let mut socket_seen = socket.exists();
    let mut ticks: u32 = 0;
    let mut chunk = [0_u8; 4096];
    'input: loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        // SAFETY: poll inspects descriptor 0 with the given timeout.
        let ready = unsafe {
            let mut pollfd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            libc::poll(&mut pollfd, 1, 50)
        };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            // About once a second: warn once when the wrapper's socket
            // goes away, so a dead wrapper is not mistaken for a quiet
            // agent. The buffer is kept either way.
            ticks += 1;
            if ticks >= 20 {
                ticks = 0;
                let present = socket.exists();
                if socket_seen && !present {
                    shared.print_line(
                        "the headless wrapper disconnected; the agent is untouched and your line is kept",
                    );
                }
                socket_seen = present;
            }
            continue;
        }
        // Read the descriptor directly: a buffered stdin reader would
        // hold bytes poll can no longer see.
        // SAFETY: read fills at most the chunk it is given.
        let read = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if read <= 0 {
            break;
        }
        for byte in &chunk[..usize::try_from(read).unwrap_or(0)] {
            let action = {
                let mut editor = shared.editor.lock().expect("editor lock poisoned");
                editor.feed(*byte)
            };
            match action {
                Action::None => {}
                Action::Changed => {
                    let mut out = shared.out.lock().expect("stdout lock poisoned");
                    let _ = shared.redraw(&mut out);
                }
                Action::Submit => {
                    if submit(manager, &socket, name, &user, &tty, &shared) {
                        break 'input;
                    }
                }
                Action::Interrupt => send_interrupt(&socket, &shared),
                Action::Hint => {
                    shared.print_line("press Ctrl-C again to interrupt");
                }
                Action::Quit => break 'input,
            }
        }
    }
    quit.store(true, Ordering::Relaxed);
    let _ = follower.join();
    shared.print_line(&format!(
        "Disconnected from {name}; the agent is still running"
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(editor: &mut LineEditor, bytes: &[u8]) -> Vec<Action> {
        bytes.iter().map(|byte| editor.feed(*byte)).collect()
    }

    #[test]
    fn typing_and_enter_submit_the_line() {
        let mut editor = LineEditor::default();
        feed(&mut editor, b"run date");
        assert!(matches!(editor.feed(b'\r'), Action::Submit));
        assert_eq!(editor.take_text(), "run date");
    }

    #[test]
    fn bracketed_paste_is_one_multiline_buffer() {
        let mut editor = LineEditor::default();
        feed(&mut editor, b"\x1b[200~first\r\nsecond\x1b[201~");
        let (display, _) = editor.display();
        assert_eq!(display, "[paste: 2 lines, 12 bytes]");
        assert!(matches!(editor.feed(b'\r'), Action::Submit));
        assert_eq!(editor.take_text(), "first\nsecond");
    }

    #[test]
    fn ctrl_c_clears_then_quits_and_ctrl_j_inserts_a_newline() {
        let mut editor = LineEditor::default();
        feed(&mut editor, b"draft");
        assert!(matches!(editor.feed(0x03), Action::Changed));
        assert!(editor.text.is_empty());
        // Empty line: first Ctrl-C only hints, the second interrupts;
        // Ctrl-C never leaves the console.
        assert!(matches!(editor.feed(0x03), Action::Hint));
        assert!(matches!(editor.feed(0x03), Action::Interrupt));
        assert!(matches!(editor.feed(0x03), Action::Hint));
        feed(&mut editor, b"one");
        assert!(matches!(editor.feed(b'\n'), Action::Changed));
        feed(&mut editor, b"two");
        assert_eq!(editor.take_text(), "one\ntwo");
    }

    #[test]
    fn ctrl_d_on_an_empty_line_quits() {
        let mut editor = LineEditor::default();
        assert!(matches!(editor.feed(0x04), Action::Quit));
    }

    #[test]
    fn editing_keys_move_and_delete() {
        let mut editor = LineEditor::default();
        feed(&mut editor, b"abc");
        feed(&mut editor, b"\x1b[D");
        feed(&mut editor, b"X");
        assert_eq!(editor.display().0, "abXc");
        feed(&mut editor, b"\x1b[3~");
        assert_eq!(editor.display().0, "abX");
        feed(&mut editor, b"\x01");
        feed(&mut editor, b"Y");
        assert_eq!(editor.display().0, "YabX");
    }
}

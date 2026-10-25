//! The tmux backend: sessions are workspaces, windows are tabs.
//!
//! tmux tracks no per-agent status, so [`PaneInfo::agent_status`] is always
//! `None` and `wait_agent_status` is unsupported; callers infer readiness
//! from the screen, as they already do for any pane whose backend reports
//! no status.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::runner::{detail, expect_ok, system_runner, CommandRunner, SharedRunner};
use crate::{
    Capabilities, CreatedPane, Multiplexer, PaneId, PaneInfo, PaneTerminal, Result, ScreenRead,
    TabId, TabInfo, TermError, WorkspaceId, WorkspaceInfo,
};

const SEP: char = '\t';
const PANE_FORMAT: &str = "#{pane_id}\t#{window_id}\t#{session_id}\t#{pane_current_path}\t#{pane_title}\t#{pane_pid}\t#{pane_current_command}";

/// A [`crate::TermBackend`] that drives tmux.
pub struct TmuxBackend {
    executable: PathBuf,
    socket: Option<String>,
    runner: SharedRunner,
}

/// Process-wide paste-buffer counter: two backends in one process must
/// never mint the same buffer name.
static PASTE_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TmuxBackend {
    /// Drive `executable` against the default server, or the `-L` socket
    /// named by `socket` (tests use a private socket per server).
    pub fn new(executable: PathBuf, socket: Option<String>) -> Self {
        Self {
            executable,
            socket,
            runner: system_runner(),
        }
    }

    /// Drive `executable` through a caller-supplied command runner (tests).
    pub fn with_runner(
        executable: PathBuf,
        socket: Option<String>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            executable,
            socket,
            runner,
        }
    }

    fn args(&self, rest: &[&str]) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(socket) = &self.socket {
            args.extend(["-L".to_owned(), socket.clone()]);
        }
        args.extend(rest.iter().map(|part| (*part).to_owned()));
        args
    }

    fn run(&self, rest: &[&str], purpose: &str) -> Result<String> {
        let output = self.runner.run(&self.executable, &self.args(rest), None)?;
        expect_ok(&output, purpose)?;
        Ok(output.stdout)
    }

    /// Like [`Self::run`], but a not-yet-running server reads as empty.
    fn run_listing(&self, rest: &[&str], purpose: &str) -> Result<String> {
        let output = self.runner.run(&self.executable, &self.args(rest), None)?;
        if output.status != 0
            && [
                "no server running",
                "failed to connect",
                "error connecting",
                "server exited",
            ]
            .iter()
            .any(|phrase| output.stderr.contains(phrase))
        {
            return Ok(String::new());
        }
        expect_ok(&output, purpose)?;
        Ok(output.stdout)
    }

    fn pane_rows(&self, workspace: Option<&WorkspaceId>) -> Result<Vec<PaneInfo>> {
        let (scope, target): (Vec<&str>, String) = match workspace {
            Some(workspace) => (vec!["-s", "-t"], workspace.to_string()),
            None => (vec!["-a"], String::new()),
        };
        let mut rest: Vec<&str> = vec!["list-panes"];
        rest.extend(&scope);
        if !target.is_empty() {
            rest.push(&target);
        }
        rest.extend(["-F", PANE_FORMAT]);
        let stdout = self.run_listing(&rest, "tmux list-panes")?;
        let mut panes: Vec<PaneInfo> = stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(parse_pane_row)
            .collect::<Result<_>>()?;
        // A pane whose foreground command is a wrapper (a shell script, an
        // interpreter, a launcher) hides the harness it started. Look under
        // the pane's shell process for one before reporting no agent.
        if panes
            .iter()
            .any(|pane| pane.agent.is_none() && pane.pid.is_some())
        {
            let processes = scan_processes();
            if !processes.is_empty() {
                for pane in &mut panes {
                    if pane.agent.is_none() {
                        if let Some(pid) = pane.pid {
                            pane.agent = detect_harness_in(pid, &processes).map(str::to_owned);
                        }
                    }
                }
            }
        }
        Ok(panes)
    }
}

fn parse_pane_row(line: &str) -> Result<PaneInfo> {
    // Parse from both ends: the title (field 5 of 7) is program-controlled
    // and may itself contain the separator, so any surplus fields belong
    // to it rather than failing the whole census.
    let fields: Vec<&str> = line.split(SEP).collect();
    if fields.len() < 7 {
        return Err(TermError::failed(format!(
            "tmux list-panes: malformed row {line:?}"
        )));
    }
    let tail = fields.len() - 2;
    let command = fields[fields.len() - 1];
    let title = fields[4..tail].join(&SEP.to_string());
    Ok(PaneInfo {
        id: fields[0].to_owned().into(),
        tab_id: Some(fields[1].to_owned().into()),
        workspace_id: Some(fields[2].to_owned().into()),
        cwd: (!fields[3].is_empty()).then(|| PathBuf::from(fields[3])),
        title: (!title.is_empty()).then_some(title),
        agent: known_harness(command).map(str::to_owned),
        agent_status: None,
        pid: fields[tail].parse::<u32>().ok(),
        terminal_id: None,
    })
}

/// Harness kinds recognisable from a pane's foreground command name.
fn known_harness(command: &str) -> Option<&str> {
    let base = command.rsplit('/').next().unwrap_or(command);
    harness_basename(base)
}

/// The harness kind a program basename names, allowing the `.real` suffix a
/// launcher binary may carry behind its public name.
fn harness_basename(base: &str) -> Option<&'static str> {
    match base.strip_suffix(".real").unwrap_or(base) {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "muse" => Some("muse"),
        _ => None,
    }
}

/// One visible process, as harness detection needs it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcEntry {
    pid: u32,
    ppid: u32,
    pgrp: u32,
    /// Foreground process group of the process's controlling terminal.
    tpgid: i64,
    /// Basename of the kernel executable image, when readable.
    exe_base: Option<String>,
    /// First arguments, when readable.
    argv: Vec<String>,
}

impl ProcEntry {
    fn harness(&self) -> Option<&'static str> {
        if let Some(kind) = self.exe_base.as_deref().and_then(harness_basename) {
            return Some(kind);
        }
        // An interpreter running a harness script names it as its first
        // file argument, not in argv[0].
        self.argv
            .iter()
            .take(2)
            .find_map(|argument| argument.rsplit('/').next().and_then(harness_basename))
    }
}

/// The harness running in a pane whose shell is `shell_pid`: the shallowest
/// known harness among the shell and its descendants that belong to the
/// terminal's foreground job. `None` when the pane runs none, or when the
/// shell is not in `processes`.
fn detect_harness_in(shell_pid: u32, processes: &[ProcEntry]) -> Option<&'static str> {
    let shell = processes.iter().find(|entry| entry.pid == shell_pid)?;
    let foreground = shell.tpgid;
    let mut children: std::collections::HashMap<u32, Vec<&ProcEntry>> =
        std::collections::HashMap::new();
    for entry in processes {
        children.entry(entry.ppid).or_default().push(entry);
    }
    // Breadth-first from the shell, so the program the shell launched wins
    // over anything that program itself later starts.
    let mut queue = std::collections::VecDeque::from([shell]);
    let mut seen = std::collections::HashSet::from([shell_pid]);
    while let Some(entry) = queue.pop_front() {
        // With no foreground group to compare against, only the shell
        // itself (the exec'd-harness case) can be named; treating every
        // descendant as foreground would promote background jobs.
        let in_foreground =
            entry.pid == shell_pid || (foreground > 0 && i64::from(entry.pgrp) == foreground);
        if in_foreground {
            if let Some(kind) = entry.harness() {
                return Some(kind);
            }
        }
        for child in children.get(&entry.pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                queue.push_back(child);
            }
        }
    }
    None
}

/// Every visible process from `/proc`; empty where `/proc` is unavailable.
fn scan_processes() -> Vec<ProcEntry> {
    #[cfg(target_os = "linux")]
    {
        scan_proc()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn scan_proc() -> Vec<ProcEntry> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        let root = entry.path();
        let Ok(stat) = std::fs::read(root.join("stat")) else {
            continue;
        };
        let Some((ppid, pgrp, tpgid)) = parse_stat_groups(&stat) else {
            continue;
        };
        let argv = std::fs::read(root.join("cmdline"))
            .map(|raw| {
                raw.split(|byte| *byte == 0)
                    .filter(|part| !part.is_empty())
                    .take(3)
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let exe_base = std::fs::read_link(root.join("exe")).ok().and_then(|path| {
            path.file_name().map(|name| {
                name.to_string_lossy()
                    .trim_end_matches(" (deleted)")
                    .to_owned()
            })
        });
        processes.push(ProcEntry {
            pid,
            ppid,
            pgrp,
            tpgid,
            exe_base,
            argv,
        });
    }
    processes
}

/// `(ppid, pgrp, tpgid)` from a `/proc/PID/stat` record, whose command name
/// is opaque bytes between the first `(` and the last `)`.
#[cfg(any(target_os = "linux", test))]
fn parse_stat_groups(stat: &[u8]) -> Option<(u32, u32, i64)> {
    let close = stat.iter().rposition(|byte| *byte == b')')?;
    let fields: Vec<&[u8]> = stat[close + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    // After the command name: state, ppid, pgrp, session, tty_nr, tpgid.
    let number = |index: usize| {
        fields
            .get(index)
            .and_then(|field| std::str::from_utf8(field).ok())
            .and_then(|field| field.parse::<i64>().ok())
    };
    Some((
        u32::try_from(number(1)?).ok()?,
        u32::try_from(number(2)?).ok()?,
        number(5)?,
    ))
}

impl Multiplexer for TmuxBackend {
    fn backend_name(&self) -> &'static str {
        "tmux"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_agent_status: false,
            native_prompt: false,
        }
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        let stdout = self.run_listing(
            &["list-sessions", "-F", "#{session_id}\t#{session_name}"],
            "tmux list-sessions",
        )?;
        stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let mut fields = line.split(SEP);
                let id = fields.next().unwrap_or_default();
                let label = fields.next().unwrap_or_default();
                Ok(WorkspaceInfo {
                    id: id.to_owned().into(),
                    label: label.to_owned(),
                })
            })
            .collect()
    }

    fn create_workspace(
        &self,
        label: &str,
        cwd: &std::path::Path,
        environment: &[String],
    ) -> Result<CreatedPane> {
        let cwd_text = cwd.display().to_string();
        let mut rest: Vec<&str> = vec![
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}\t#{window_id}\t#{pane_id}",
            "-s",
            label,
            "-c",
            &cwd_text,
        ];
        for entry in environment {
            rest.extend(["-e", entry]);
        }
        parse_created(&self.run(&rest, "tmux new-session")?, "tmux new-session")
    }

    fn focus_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        // Focusing a session is client-scoped in tmux; with no attached
        // client there is nothing to move, which is not a failure.
        let args = self.args(&["switch-client", "-t", workspace.as_str()]);
        let output = self.runner.run(&self.executable, &args, None)?;
        if output.status == 0 || output.stderr.contains("no current client") {
            return Ok(());
        }
        Err(TermError::failed(format!(
            "tmux switch-client: {}",
            detail(&output)
        )))
    }

    fn rename_workspace(&self, workspace: &WorkspaceId, label: &str) -> Result<()> {
        self.run(
            &["rename-session", "-t", workspace.as_str(), label],
            "tmux rename-session",
        )?;
        Ok(())
    }

    fn close_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        self.run(
            &["kill-session", "-t", workspace.as_str()],
            "tmux kill-session",
        )?;
        Ok(())
    }

    fn list_tabs(&self, workspace: &WorkspaceId) -> Result<Vec<TabInfo>> {
        let stdout = self.run(
            &[
                "list-windows",
                "-t",
                workspace.as_str(),
                "-F",
                "#{window_id}\t#{window_name}",
            ],
            "tmux list-windows",
        )?;
        stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let mut fields = line.split(SEP);
                Ok(TabInfo {
                    id: fields.next().unwrap_or_default().to_owned().into(),
                    workspace_id: workspace.clone(),
                    label: fields.next().unwrap_or_default().to_owned(),
                })
            })
            .collect()
    }

    fn create_tab(
        &self,
        workspace: &WorkspaceId,
        label: &str,
        cwd: &std::path::Path,
        environment: &[String],
    ) -> Result<CreatedPane> {
        let cwd_text = cwd.display().to_string();
        let mut rest: Vec<&str> = vec![
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{session_id}\t#{window_id}\t#{pane_id}",
            "-t",
            workspace.as_str(),
            "-n",
            label,
            "-c",
            &cwd_text,
        ];
        for entry in environment {
            rest.extend(["-e", entry]);
        }
        let created = parse_created(&self.run(&rest, "tmux new-window")?, "tmux new-window")?;
        if created.workspace_id.as_str() != workspace.as_str() {
            return Err(TermError::failed(
                "tmux new-window: created window belongs to a different session",
            ));
        }
        Ok(created)
    }

    fn focus_tab(&self, tab: &TabId) -> Result<()> {
        self.run(&["select-window", "-t", tab.as_str()], "tmux select-window")?;
        Ok(())
    }

    fn rename_tab(&self, tab: &TabId, label: &str) -> Result<()> {
        self.run(
            &["rename-window", "-t", tab.as_str(), label],
            "tmux rename-window",
        )?;
        Ok(())
    }

    fn close_tab(&self, tab: &TabId) -> Result<()> {
        self.run(&["kill-window", "-t", tab.as_str()], "tmux kill-window")?;
        Ok(())
    }

    fn list_panes(&self, workspace: Option<&WorkspaceId>) -> Result<Vec<PaneInfo>> {
        self.pane_rows(workspace)
    }

    fn focus_pane(&self, pane: &PaneId) -> Result<()> {
        self.run(&["select-pane", "-t", pane.as_str()], "tmux select-pane")?;
        Ok(())
    }

    fn close_pane(&self, pane: &PaneId) -> Result<()> {
        self.run(&["kill-pane", "-t", pane.as_str()], "tmux kill-pane")?;
        Ok(())
    }
}

impl PaneTerminal for TmuxBackend {
    fn pane_info(&self, pane: &PaneId) -> Result<PaneInfo> {
        self.pane_rows(None)?
            .into_iter()
            .find(|info| &info.id == pane)
            .ok_or_else(|| TermError::NotFound(format!("tmux pane {pane} not found")))
    }

    fn read_screen(&self, pane: &PaneId, read: ScreenRead) -> Result<String> {
        let start;
        let mut rest: Vec<&str> = vec!["capture-pane", "-p"];
        if read.ansi {
            rest.push("-e");
        }
        rest.extend(["-t", pane.as_str()]);
        match read.scrollback_lines {
            Some(0) => {}
            Some(lines) => {
                start = format!("-{lines}");
                rest.extend(["-S", &start]);
            }
            None => rest.extend(["-S", "-"]),
        }
        self.run(&rest, "tmux capture-pane")
    }

    fn paste_text(&self, pane: &PaneId, text: &str) -> Result<()> {
        // Text that already carries escape framing (a verified-submission
        // layer wraps prompts in bracketed-paste markers itself) must reach
        // the pane byte-for-byte: paste-buffer sanitises ESC bytes into
        // literal `^[` text, while `send-keys -l` delivers them raw.
        if text.contains('\u{1b}') {
            let args = self.args(&["send-keys", "-t", pane.as_str(), "-l", "--", text]);
            return expect_ok(
                &self.runner.run(&self.executable, &args, None)?,
                "tmux send-keys",
            );
        }
        let buffer = format!(
            "term-backend-{}-{}",
            std::process::id(),
            PASTE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let load = self.args(&["load-buffer", "-b", &buffer, "-"]);
        expect_ok(
            &self.runner.run(&self.executable, &load, Some(text))?,
            "tmux load-buffer",
        )?;
        // -p uses bracketed paste where the pane's program enabled it; -d
        // deletes the buffer once pasted. On failure, delete it explicitly
        // so prompt text never lingers in a server-side buffer.
        let paste = self.run(
            &[
                "paste-buffer",
                "-p",
                "-d",
                "-b",
                &buffer,
                "-t",
                pane.as_str(),
            ],
            "tmux paste-buffer",
        );
        if paste.is_err() {
            let _ = self.runner.run(
                &self.executable,
                &self.args(&["delete-buffer", "-b", &buffer]),
                None,
            );
        }
        paste.map(|_| ())
    }

    fn send_keys(&self, pane: &PaneId, keys: &str) -> Result<()> {
        let key = match keys {
            "Backspace" => "BSpace",
            other => other,
        };
        self.run(&["send-keys", "-t", pane.as_str(), key], "tmux send-keys")?;
        Ok(())
    }
}

fn parse_created(stdout: &str, purpose: &str) -> Result<CreatedPane> {
    let line = stdout.trim();
    let fields: Vec<&str> = line.split(SEP).collect();
    if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) {
        return Err(TermError::failed(format!(
            "{purpose}: malformed identifier row {line:?}"
        )));
    }
    Ok(CreatedPane {
        workspace_id: fields[0].to_owned().into(),
        tab_id: fields[1].to_owned().into(),
        pane_id: fields[2].to_owned().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Scripted {
        calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
        responses: Mutex<Vec<String>>,
    }

    impl CommandRunner for Scripted {
        fn run(
            &self,
            _program: &std::path::Path,
            args: &[String],
            stdin: Option<&str>,
        ) -> Result<CommandOutput> {
            self.calls
                .lock()
                .unwrap()
                .push((args.to_vec(), stdin.map(str::to_owned)));
            let stdout = self
                .responses
                .lock()
                .unwrap()
                .drain(..1)
                .next()
                .unwrap_or_default();
            Ok(CommandOutput {
                status: 0,
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn backend(responses: Vec<&str>) -> (TmuxBackend, Arc<Scripted>) {
        let scripted = Arc::new(Scripted {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
        });
        (
            TmuxBackend::with_runner(
                PathBuf::from("tmux"),
                Some("test-sock".to_owned()),
                scripted.clone(),
            ),
            scripted,
        )
    }

    #[test]
    fn list_panes_parses_ids_cwd_title_and_pid() {
        let (backend, scripted) = backend(vec!["%1\t@1\t$1\t/work\tshell title\t4321\tcodex\n"]);
        let panes = backend.list_panes(None).unwrap();
        assert_eq!(panes[0].id.as_str(), "%1");
        assert_eq!(panes[0].cwd.as_deref(), Some(std::path::Path::new("/work")));
        assert_eq!(panes[0].pid, Some(4321));
        assert_eq!(panes[0].agent.as_deref(), Some("codex"));
        assert_eq!(panes[0].agent_status, None);
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls[0].0[..4], ["-L", "test-sock", "list-panes", "-a"]);
    }

    #[test]
    fn read_and_paste_use_capture_and_buffers() {
        let (backend, scripted) = backend(vec!["screen text\n", "", ""]);
        let pane = PaneId::from("%1");
        let text = backend
            .read_screen(
                &pane,
                ScreenRead {
                    scrollback_lines: Some(50),
                    ansi: true,
                },
            )
            .unwrap();
        assert_eq!(text, "screen text\n");
        backend.paste_text(&pane, "typed text").unwrap();
        let calls = scripted.calls.lock().unwrap();
        assert!(calls[0]
            .0
            .windows(2)
            .any(|pair| pair == ["capture-pane", "-p"]));
        assert!(calls[0].0.contains(&"-e".to_owned()));
        assert!(calls[0].0.windows(2).any(|pair| pair == ["-S", "-50"]));
        assert!(calls[1].0.contains(&"load-buffer".to_owned()));
        assert_eq!(calls[1].1.as_deref(), Some("typed text"));
        assert!(calls[2].0.contains(&"paste-buffer".to_owned()));
        assert!(calls[2].0.contains(&"-p".to_owned()));
    }

    #[test]
    fn escape_framed_text_goes_as_raw_literal_keys_never_a_paste_buffer() {
        let (backend, scripted) = backend(vec![""]);
        backend
            .paste_text(&PaneId::from("%1"), "\u{1b}[200~framed\u{1b}[201~")
            .unwrap();
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.contains(&"send-keys".to_owned()));
        assert!(calls[0].0.contains(&"-l".to_owned()));
        assert_eq!(
            calls[0].0.last().map(String::as_str),
            Some("\u{1b}[200~framed\u{1b}[201~")
        );
    }

    #[test]
    fn a_tab_inside_a_pane_title_does_not_fail_the_census() {
        let (backend, _scripted) =
            backend(vec!["%1\t@1\t$1\t/work\ttitle\twith\ttabs\t99\tbash\n"]);
        let panes = backend.list_panes(None).unwrap();
        assert_eq!(panes[0].title.as_deref(), Some("title\twith\ttabs"));
        assert_eq!(panes[0].pid, Some(99));
    }

    fn proc(pid: u32, ppid: u32, pgrp: u32, exe: Option<&str>, argv: &[&str]) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            pgrp,
            tpgid: 200,
            exe_base: exe.map(str::to_owned),
            argv: argv.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    #[test]
    fn wrapped_launch_is_detected_under_the_pane_shell() {
        // shell(100) -> bash wrapper(200, fg leader) -> python script "claude"(201)
        let processes = vec![
            proc(100, 1, 100, Some("bash"), &["-bash"]),
            proc(200, 100, 200, Some("bash"), &["bash", "/opt/wrap.sh"]),
            proc(
                201,
                200,
                200,
                Some("python3"),
                &["python3", "/opt/bin/claude"],
            ),
            // A background descendant in another group is not the pane's job.
            proc(300, 100, 300, Some("codex"), &["codex"]),
        ];
        assert_eq!(detect_harness_in(100, &processes), Some("claude"));
        // The shallowest foreground harness wins over one it started itself.
        let nested = vec![
            proc(100, 1, 100, Some("bash"), &["-bash"]),
            proc(200, 100, 200, Some("muse.real"), &["/opt/muse.real"]),
            proc(201, 200, 200, Some("codex"), &["codex"]),
        ];
        assert_eq!(detect_harness_in(100, &nested), Some("muse"));
        assert_eq!(detect_harness_in(999, &nested), None);
        let idle = vec![proc(100, 1, 100, Some("bash"), &["-bash"])];
        assert_eq!(detect_harness_in(100, &idle), None);
    }

    #[test]
    fn stat_groups_survive_spaces_and_parens_in_the_command_name() {
        let stat = b"4242 (weird ) name) S 100 200 200 34816 200 4194304 100 0 0 0";
        assert_eq!(parse_stat_groups(stat), Some((100, 200, 200)));
    }

    #[test]
    fn prompt_is_paste_then_enter_without_a_native_primitive() {
        let (backend, scripted) = backend(vec!["", "", ""]);
        backend.prompt(&PaneId::from("%1"), "go").unwrap();
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls[2]
            .0
            .windows(2)
            .any(|pair| pair == ["send-keys", "-t"]));
        assert_eq!(calls[2].0.last().map(String::as_str), Some("Enter"));
    }
}

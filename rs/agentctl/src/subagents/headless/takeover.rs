//! `agentctl takeover NAME` and `agentctl handback NAME` (`#591 headless-takeover`): moving a
//! headless agent's session to an interactive Claude Code in a Herdr tab and back.
//!
//! One session must never have two writers. A writer is any process whose command line runs
//! Claude Code on the session (`--resume SESSION` or `--session-id SESSION`); that is checked
//! directly ([`wrapper::session_processes`]), because a launcher can exit while the real harness
//! it started keeps the session open. The guards:
//!
//! - **The takeover mark** in the record (`headless.takeover`) is written before the wrapper is
//!   asked to stop and removed only by `handback`. A wrapper exits 0 at once while it is set
//!   (so a supervisor does not loop), and `stop` refuses.
//! - **The wrapper holds deliveries** while `takeover` waits for the running turn to end, so no
//!   new turn starts between that check and the stop.
//! - **`takeover-hold`**, the tab's command, holds `headless.lock` while the interactive harness
//!   runs, refuses to start one while any writer of the session lives, and when it exits ends
//!   everything left of it (its process group, then any writer of the session) before it
//!   records `lifecycle.released` and releases the lock.
//! - **The wrapper** refuses to start beside a writer of the session, and when its own harness
//!   exits it ends the rest of that harness's process group and any writer left.
//! - **`handback`** refuses while a takeover is still stopping the wrapper or a wrapper answers,
//!   ends the interactive session's writers (SIGTERM, then SIGKILL after half its wait), and
//!   starts the wrapper only once no writer is left and the lock is free.

use serde_json::Map;

use super::*;

/// Schema of the takeover mark.
const TAKEOVER_SCHEMA: &str = "agentctl-headless-takeover/v1";
/// How long `takeover` waits for the tab's holder to start the interactive harness.
const HOLD_START: Duration = Duration::from_secs(30);

/// Options of `agentctl takeover`.
#[derive(Clone, Debug)]
pub struct TakeoverOptions {
    /// Herdr workspace for the new tab; the caller's own (`HERDR_WORKSPACE_ID`) otherwise.
    pub workspace: Option<String>,
    /// Interrupt a running turn instead of waiting for it to end.
    pub interrupt: bool,
    /// Longest wait for the running turn to end.
    pub wait: Duration,
    /// Claude Code executable for the interactive session; by default the one the wrapper ran,
    /// else `claude` on the pane's PATH.
    pub claude_bin: Option<PathBuf>,
}

/// The takeover mark of a record, when it has one.
pub(super) fn takeover_marker(record: &AgentRecord) -> Option<&Value> {
    record
        .extra
        .get("headless")
        .and_then(|spec| spec.get("takeover"))
        .filter(|marker| marker["schema"] == TAKEOVER_SCHEMA)
}

/// Whether a takeover is over and only waits for `handback`: its holder recorded the end or a
/// failure, or the holder is gone without recording either.
pub(super) fn takeover_needs_handback(marker: &Value) -> bool {
    match marker["state"].as_str() {
        Some("ended" | "failed") => true,
        Some("starting" | "interactive") => marker
            .get("holder_identity")
            .and_then(|identity| {
                serde_json::from_value::<agent_queue::identity::ProcessIdentity>(identity.clone())
                    .ok()
            })
            .is_some_and(|identity| {
                agent_queue::identity::liveness(&identity)
                    == agent_queue::identity::ProcessLiveness::Dead
            }),
        _ => false,
    }
}

fn set_marker(record: &mut AgentRecord, marker: Option<Value>) {
    if let Some(spec) = record
        .extra
        .get_mut("headless")
        .and_then(Value::as_object_mut)
    {
        match marker {
            Some(marker) => {
                spec.insert("takeover".to_owned(), marker);
            }
            None => {
                spec.remove("takeover");
            }
        }
    }
}

/// The systemd user unit a process runs in, from its cgroup, when it is a headless unit.
fn systemd_unit(pid: u64) -> Option<String> {
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    cgroup
        .lines()
        .flat_map(|line| line.rsplit('/'))
        .find(|part| part.starts_with("agentctl-headless-") && part.ends_with(".service"))
        .map(str::to_owned)
}

/// One variable of a process's environment.
fn environment_of(pid: u64, name: &str) -> Option<String> {
    let environ = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    environ
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .find_map(|entry| entry.strip_prefix(&prefix).map(str::to_owned))
}

/// The record's harness arguments for an interactive session: the wrapper's
/// `--permission-prompts` choice is dropped, since a human is there to answer.
fn interactive_arguments(arguments: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut skip = false;
    for argument in arguments {
        if skip {
            skip = false;
            continue;
        }
        if argument == "--permission-prompts" {
            skip = true;
            continue;
        }
        if argument.starts_with("--permission-prompts=") {
            continue;
        }
        kept.push(argument.clone());
    }
    kept
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// A line on standard error that never panics, even when the tab's terminal is gone.
fn note(message: &str) {
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        format!("agentctl: {message}\n").as_bytes(),
    );
}

/// Why stopping the wrapper failed, and whether it had already been asked to stop.
struct StopFailure {
    stop_requested: bool,
    error: AgentError,
}

fn stop_failure(stop_requested: bool, message: impl Into<String>) -> StopFailure {
    StopFailure {
        stop_requested,
        error: fail(message.into()),
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Change the takeover mark `marker_token` of the record `record_token` under the record
    /// lock (`None` clears it). Refuses when the record or the mark was replaced or cleared, so
    /// no step of one takeover can touch another's.
    fn change_marker(
        &self,
        agent_name: &str,
        record_token: &str,
        marker_token: &str,
        change: impl FnOnce(&mut Map<String, Value>) -> bool,
    ) -> Result<()> {
        let _lock = self.lock_with_runtime(
            agent_name,
            &agent::GraceRuntime::new(Duration::from_secs(10)),
        )?;
        let mut record = self.load(agent_name)?;
        if record.token != record_token {
            return Err(fail(format!("agent {agent_name:?} was replaced")));
        }
        let Some(mut marker) = takeover_marker(&record)
            .filter(|marker| marker["token"] == marker_token)
            .and_then(Value::as_object)
            .cloned()
        else {
            return Err(fail(format!(
                "the takeover mark of {agent_name:?} was cleared or replaced"
            )));
        };
        let keep = change(&mut marker);
        set_marker(&mut record, keep.then_some(Value::Object(marker)));
        self.save(&record)
    }

    /// Record that the takeover `marker_token` failed, with why; `handback` recovers from it.
    fn mark_failed(&self, agent_name: &str, record_token: &str, marker_token: &str, cause: &str) {
        let _ = self.change_marker(agent_name, record_token, marker_token, |marker| {
            marker.insert("state".to_owned(), json!("failed"));
            marker.insert("cause".to_owned(), json!(cause));
            marker.insert("ended_at".to_owned(), json!(now_seconds()));
            true
        });
    }

    /// Stop a headless agent's wrapper at a turn boundary and open its session in an
    /// interactive Claude Code in a new Herdr tab.
    pub fn takeover_headless(&self, agent_name: &str, options: &TakeoverOptions) -> Result<Value> {
        let record = self.load(agent_name)?;
        if !record.is_headless() || record.lifecycle != "running" {
            return Err(fail(format!(
                "agent {agent_name:?} is not a running headless agent"
            )));
        }
        if let Some(marker) = takeover_marker(&record) {
            return Err(fail(format!(
                "agent {agent_name:?} is already taken over (tab {}); use agentctl handback {agent_name}",
                marker["tab"].as_str().unwrap_or("?")
            )));
        }
        let session = record.headless_session()?;
        let workspace = options
            .workspace
            .clone()
            .or_else(|| self.inherited_workspace.clone())
            .ok_or_else(|| {
                fail("takeover needs a Herdr workspace for the tab: pass --workspace ID or run it inside Herdr")
            })?;
        let directory = self.directory(agent_name)?;
        let socket = hcontrol::socket_path(&directory);
        let status = hcontrol::request(&socket, &json!({"op": "status"}), CONTROL_TIMEOUT).ok();
        let wrapper_pid = status
            .as_ref()
            .and_then(|status| status["wrapper_pid"].as_u64());
        let harness_pid = status
            .as_ref()
            .and_then(|status| status["harness_pid"].as_u64());
        // The interactive session must find the same transcript: same working directory and the
        // wrapper's Claude Code configuration directory.
        let config_directory = wrapper_pid
            .and_then(|pid| environment_of(pid, "CLAUDE_CONFIG_DIR"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let effective_config = config_directory
            .clone()
            .or_else(claude::config_directory)
            .ok_or_else(|| fail("cannot tell Claude Code's configuration directory"))?;
        let transcript =
            claude::transcript_path(Path::new(&record.cwd), &session, &effective_config);
        if !transcript.is_file() {
            return Err(fail(format!(
                "no Claude Code transcript for session {session} at {}; nothing to resume",
                transcript.display()
            )));
        }
        // The executable the wrapper started, when it named one by absolute path (its argv[0];
        // `/proc/PID/exe` could be an interpreter); otherwise `claude` on the pane's PATH.
        let claude_bin = options.claude_bin.clone().or_else(|| {
            harness_pid
                .and_then(|pid| fs::read(format!("/proc/{pid}/cmdline")).ok())
                .and_then(|cmdline| {
                    cmdline
                        .split(|byte| *byte == 0)
                        .next()
                        .and_then(|first| std::str::from_utf8(first).ok())
                        .map(PathBuf::from)
                })
                .filter(|path| path.is_absolute())
        });
        let unit = wrapper_pid.and_then(systemd_unit);
        let token = claude::random_uuid().map_err(|error| fail(error.to_string()))?;
        let marker = json!({
            "schema": TAKEOVER_SCHEMA,
            "token": token,
            "state": "stopping",
            "since": now_seconds(),
            "systemd_unit": unit,
            "claude_config_dir": config_directory,
            "claude_bin": claude_bin,
            "workspace": workspace,
        });
        // The mark goes in first: from here no supervisor can start a wrapper.
        {
            let _lock = self.lock(agent_name)?;
            let mut current = self.load(agent_name)?;
            if current.token != record.token || takeover_marker(&current).is_some() {
                return Err(fail(format!(
                    "agent {agent_name:?} changed or was taken over meanwhile"
                )));
            }
            set_marker(&mut current, Some(marker));
            self.save(&current)?;
        }
        let stopped = match self.stop_wrapper_for_takeover(&directory, options) {
            Ok(stopped) => stopped,
            Err(failure) if !failure.stop_requested => {
                // The wrapper still serves the agent: undo the mark.
                let _ = self.change_marker(agent_name, &record.token, &token, |_| false);
                return Err(failure.error);
            }
            Err(failure) => {
                let cause = failure.error.to_string();
                self.mark_failed(agent_name, &record.token, &token, &cause);
                return Err(fail(format!(
                    "{cause}; {agent_name} stays marked taken over: run agentctl handback {agent_name} once its wrapper has exited"
                )));
            }
        };
        let writers = wrapper::session_processes(&session);
        if !writers.is_empty() {
            let cause = format!(
                "process(es) {writers:?} still run Claude Code on session {session} after the wrapper stopped"
            );
            self.mark_failed(agent_name, &record.token, &token, &cause);
            return Err(fail(format!(
                "{cause}; refusing a second writer. agentctl handback {agent_name} ends them and resumes headless"
            )));
        }
        let mut environment = vec![format!("AGENTCTL_AGENT={agent_name}")];
        if let Some(config) = &config_directory {
            environment.push(format!("CLAUDE_CONFIG_DIR={}", config.display()));
        }
        let label = format!("{agent_name}-tui");
        let (tab, pane) = match self.client.create_tab_with_pane(
            &workspace,
            &label,
            &record.cwd,
            &environment,
        ) {
            Ok(created) => created,
            Err(error) => {
                let cause = format!("the takeover tab could not be created: {error}");
                self.mark_failed(agent_name, &record.token, &token, &cause);
                return Err(fail(format!(
                    "{cause}; the wrapper is stopped: run agentctl handback {agent_name} to resume headless"
                )));
            }
        };
        self.change_marker(agent_name, &record.token, &token, |marker| {
            marker.insert("tab".to_owned(), json!(tab));
            marker.insert("pane".to_owned(), json!(pane));
            marker.insert("state".to_owned(), json!("starting"));
            true
        })?;
        let executable = std::env::current_exe()
            .map_err(|error| fail(format!("locate this agentctl: {error}")))?;
        let mut arguments = vec![
            "--registry".to_owned(),
            self.registry.display().to_string(),
            "takeover-hold".to_owned(),
            agent_name.to_owned(),
            "--token".to_owned(),
            token.clone(),
        ];
        if let Some(bin) = &claude_bin {
            arguments.extend(["--claude-bin".to_owned(), bin.display().to_string()]);
        }
        let hold = self
            .client
            .start_pane_command(&pane, &executable, &arguments, HOLD_START)
            .map_err(|error| {
                fail(format!(
                    "the takeover tab {tab} did not start its holder: {error}; read the pane, then agentctl handback {agent_name}"
                ))
            })?;
        let deadline = Instant::now() + HOLD_START;
        let interactive = loop {
            let current = self.load(agent_name)?;
            let marker = takeover_marker(&current)
                .filter(|marker| marker["token"] == token)
                .cloned()
                .unwrap_or_default();
            match marker["state"].as_str() {
                Some("interactive") => break marker,
                Some("ended" | "failed") | None => {
                    return Err(fail(format!(
                        "the interactive session in tab {tab} did not run: {}; read the pane, then agentctl handback {agent_name}",
                        marker["cause"].as_str().unwrap_or("it exited at once")
                    )))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(fail(format!(
                    "the interactive session in tab {tab} did not start within {} s; read the pane, then agentctl handback {agent_name}",
                    HOLD_START.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        Ok(json!({
            "name": agent_name,
            "session": session,
            "taken_over": true,
            "wrapper_stopped": stopped,
            "systemd_unit": unit,
            "tab": tab,
            "pane": pane,
            "holder_pid": hold.pid,
            "interactive_pid": interactive["interactive_identity"]["pid"],
            "transcript": transcript,
            "next": format!("agentctl handback {agent_name}"),
        }))
    }

    /// Hold the wrapper's deliveries, wait for the running turn to end (interrupting it when
    /// asked), then stop the wrapper and wait for it to release the headless lock. `false` when
    /// no wrapper was running.
    fn stop_wrapper_for_takeover(
        &self,
        directory: &Path,
        options: &TakeoverOptions,
    ) -> std::result::Result<bool, StopFailure> {
        let socket = hcontrol::socket_path(directory);
        let ask = |op: &str| hcontrol::request(&socket, &json!({"op": op}), CONTROL_TIMEOUT);
        match ask("status") {
            Err(hcontrol::ClientError::NotRunning(_)) => {
                // No wrapper: the lock must be free, or something else holds the session.
                return match wrapper::try_lock(directory) {
                    Ok(Some(_)) => Ok(false),
                    Ok(None) => Err(stop_failure(
                        false,
                        "the headless lock is held but no wrapper answers; refusing",
                    )),
                    Err(error) => Err(stop_failure(false, error.to_string())),
                };
            }
            Err(error) => {
                return Err(stop_failure(
                    false,
                    format!("the wrapper does not answer: {error}"),
                ))
            }
            Ok(_) => {}
        }
        // A wrapper older than holds answers "unknown op"; it is then only watched for idleness.
        let held = ask("hold").is_ok_and(|answer| answer["ok"] == true);
        let release = || {
            if held {
                let _ = ask("release");
            }
        };
        if options.interrupt {
            let _ = ask("interrupt");
        }
        let deadline = Instant::now() + options.wait;
        loop {
            let status = match ask("status") {
                Ok(status) => status,
                Err(error) => {
                    release();
                    return Err(stop_failure(false, error.to_string()));
                }
            };
            if status["busy"] != true && status["current"].is_null() {
                break;
            }
            if Instant::now() >= deadline {
                release();
                return Err(stop_failure(
                    false,
                    format!(
                        "a turn is still running after {} s; pass --interrupt, or a longer --wait",
                        options.wait.as_secs()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if let Err(error) = ask("stop") {
            release();
            return Err(stop_failure(
                false,
                format!("ask the wrapper to stop: {error}"),
            ));
        }
        let deadline = Instant::now() + STOP_WAIT;
        loop {
            match wrapper::try_lock(directory) {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => {}
                Err(error) => return Err(stop_failure(true, error.to_string())),
            }
            if Instant::now() >= deadline {
                return Err(stop_failure(
                    true,
                    format!(
                        "the wrapper still runs {} s after it was asked to stop",
                        STOP_WAIT.as_secs()
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The tab's command during a takeover: hold the headless lock, run the interactive Claude
    /// Code on the agent's session in its own process group with the terminal, and when it
    /// exits end whatever is left of it, record the end, and release the lock. Returns the
    /// interactive harness's exit code.
    pub fn run_takeover_hold(
        &self,
        agent_name: &str,
        token: &str,
        claude_bin: Option<&Path>,
    ) -> Result<i32> {
        let record = self.load(agent_name)?;
        if takeover_marker(&record).is_none_or(|marker| marker["token"] != token) {
            return Err(fail(format!(
                "agent {agent_name:?} has no takeover with this token"
            )));
        }
        let failed = |cause: String| -> AgentError {
            self.mark_failed(agent_name, &record.token, token, &cause);
            fail(cause)
        };
        let session = record.headless_session()?;
        let directory = self.directory(agent_name)?;
        let Some(lock) = wrapper::try_lock(&directory)? else {
            return Err(failed(
                "the headless lock is held: a wrapper or another takeover runs".to_owned(),
            ));
        };
        if let Some(pid) = wrapper::live_previous_harness(&directory) {
            return Err(failed(format!(
                "a harness of this session (pid {pid}) is still running; refusing a second writer"
            )));
        }
        let writers = wrapper::session_processes(&session);
        if !writers.is_empty() {
            return Err(failed(format!(
                "process(es) {writers:?} already run Claude Code on session {session}; refusing a second writer"
            )));
        }
        let mut writer = events::EventWriter::open(&events::directory(&directory), agent_name)
            .map_err(|error| failed(format!("open the event log: {error}")))?;
        writer.set_session(Some(session.clone()));
        let pane = record
            .extra
            .get("headless")
            .and_then(|spec| spec.get("takeover"))
            .cloned()
            .unwrap_or_default();
        writer
            .append(
                "lifecycle.takeover",
                None,
                json!({"tab": pane["tab"], "pane": pane["pane"], "holder_pid": std::process::id()}),
            )
            .map_err(|error| failed(error.to_string()))?;
        const JOB_SIGNALS: [libc::c_int; 7] = [
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTSTP,
            libc::SIGTTIN,
            libc::SIGTTOU,
            libc::SIGTERM,
            libc::SIGHUP,
        ];
        let program = claude_bin.map_or_else(|| PathBuf::from("claude"), Path::to_path_buf);
        let mut command = std::process::Command::new(&program);
        command
            .arg("--resume")
            .arg(&session)
            .args(interactive_arguments(&record.arguments))
            .env("AGENTCTL_AGENT", agent_name)
            .current_dir(&record.cwd);
        // SAFETY: setpgid and signal are async-signal-safe and touch no memory of this process.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut command, move || {
                // Its own process group, so that everything it starts can be ended with it.
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                for signal in JOB_SIGNALS {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        // The holder only waits: it survives the tab closing (SIGHUP) and a SIGTERM to the
        // session, so that it always ends what is left and records the end.
        // SAFETY: ignoring signals in this process changes no memory.
        unsafe {
            for signal in JOB_SIGNALS {
                libc::signal(signal, libc::SIG_IGN);
            }
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let cause = format!("could not start {}: {error}", program.display());
                let _ = writer.append("lifecycle.released", None, json!({"cause": cause}));
                return Err(failed(cause));
            }
        };
        let pid = child.id();
        let group = pid as libc::pid_t;
        // SAFETY: setpgid, isatty and tcsetpgrp change only kernel state of our own child, its
        // group and our terminal.
        let terminal = unsafe {
            libc::setpgid(group, group);
            libc::isatty(libc::STDIN_FILENO) == 1
        };
        if terminal {
            // SAFETY: as above; SIGTTOU is ignored, so a background holder may do this.
            unsafe {
                libc::tcsetpgrp(libc::STDIN_FILENO, group);
            }
        }
        wrapper::record_harness_file(&directory, pid);
        let holder_identity = agent_queue::identity::observe(u64::from(std::process::id()))
            .ok()
            .and_then(|live| serde_json::to_value(live.identity).ok());
        let interactive_identity = agent_queue::identity::observe(u64::from(pid))
            .ok()
            .and_then(|live| serde_json::to_value(live.identity).ok());
        // A record that cannot be updated now must not end the session: keep waiting.
        if let Err(error) = self.change_marker(agent_name, &record.token, token, |marker| {
            marker.insert("state".to_owned(), json!("interactive"));
            marker.insert("holder_identity".to_owned(), json!(holder_identity));
            marker.insert(
                "interactive_identity".to_owned(),
                json!(interactive_identity),
            );
            marker.insert("interactive_group".to_owned(), json!(pid));
            true
        }) {
            note(&format!(
                "could not record the interactive session: {error}"
            ));
        }
        let mut code = None;
        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: waitpid writes only into `status`.
            let waited = unsafe { libc::waitpid(group, &mut status, libc::WUNTRACED) };
            if waited == -1 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if libc::WIFSTOPPED(status) {
                // A suspended harness would freeze the tab: the holder's shell never sees it.
                // SAFETY: as above.
                unsafe {
                    if terminal {
                        libc::tcsetpgrp(libc::STDIN_FILENO, group);
                    }
                    libc::kill(-group, libc::SIGCONT);
                }
                continue;
            }
            if libc::WIFEXITED(status) {
                code = Some(libc::WEXITSTATUS(status));
            }
            break;
        }
        drop(child);
        if terminal {
            // SAFETY: taking our terminal back for our own process group.
            unsafe {
                libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
            }
        }
        // A launcher can exit while the real harness it started keeps the session open: end
        // the rest of its group, then any writer of the session, before recording the end.
        let lingering = wrapper::session_processes(&session);
        wrapper::end_process_group(pid, Duration::from_secs(10));
        let writers = wrapper::session_processes(&session);
        if !writers.is_empty() {
            wrapper::end_processes(&writers, Duration::from_secs(5));
        }
        wrapper::clear_harness_file(&directory);
        let _ = writer.append(
            "lifecycle.released",
            None,
            json!({"cause": "interactive session ended", "code": code, "ended_leftovers": lingering}),
        );
        let _ = self.change_marker(agent_name, &record.token, token, |marker| {
            marker.insert("state".to_owned(), json!("ended"));
            marker.insert("ended_at".to_owned(), json!(now_seconds()));
            marker.insert("code".to_owned(), json!(code));
            true
        });
        drop(writer);
        drop(lock);
        note(&format!(
            "the interactive session of {agent_name} ended; run `agentctl handback {agent_name}` to resume it headless"
        ));
        Ok(code.unwrap_or(1))
    }

    /// End a takeover: end the interactive session, close its tab, and start the wrapper again
    /// on the same session (through its systemd unit when it ran under one).
    pub fn handback_headless(&self, agent_name: &str, wait: Duration) -> Result<Value> {
        let record = self.load(agent_name)?;
        let marker = takeover_marker(&record)
            .cloned()
            .ok_or_else(|| fail(format!("agent {agent_name:?} is not taken over")))?;
        let token = marker["token"].as_str().unwrap_or_default().to_owned();
        if marker["state"] == "stopping" {
            return Err(fail(format!(
                "a takeover of {agent_name:?} is still stopping its wrapper; wait for it to finish"
            )));
        }
        let directory = self.directory(agent_name)?;
        let socket = hcontrol::socket_path(&directory);
        if hcontrol::request(&socket, &json!({"op": "status"}), CONTROL_TIMEOUT).is_ok() {
            return Err(fail(format!(
                "a wrapper answers for {agent_name:?} while it is marked taken over; stop it (agentctl stop refuses: signal its process) before handback"
            )));
        }
        let session = record.headless_session()?;
        let started = Instant::now();
        let mut terminated = false;
        let mut killed = false;
        let lock = loop {
            let writers = wrapper::session_processes(&session);
            if writers.is_empty() {
                // The holder keeps the lock until it has recorded the end.
                if let Some(lock) = wrapper::try_lock(&directory)? {
                    break lock;
                }
            } else if !terminated {
                for pid in &writers {
                    // SAFETY: kill only sends a signal to a writer of this session.
                    unsafe {
                        libc::kill(*pid as libc::pid_t, libc::SIGTERM);
                    }
                }
                terminated = true;
            } else if !killed && started.elapsed() >= wait / 2 {
                for pid in &writers {
                    // SAFETY: kill only sends a signal to a writer of this session.
                    unsafe {
                        libc::kill(*pid as libc::pid_t, libc::SIGKILL);
                    }
                }
                killed = true;
            }
            if started.elapsed() >= wait {
                return Err(fail(format!(
                    "the interactive session of {agent_name:?} is still running after {} s (writers {writers:?}; the lock is {}); exit it, then run handback again",
                    wait.as_secs(),
                    if wrapper::try_lock(&directory)?.is_some() { "free" } else { "held by its holder" }
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let tab_closed = marker["tab"]
            .as_str()
            .is_some_and(|tab| self.client.close_tab(tab).is_ok());
        self.change_marker(agent_name, &record.token, &token, |_| false)?;
        drop(lock);
        let unit = marker["systemd_unit"].as_str().map(str::to_owned);
        let mut unit_error = None;
        if let Some(unit) = &unit {
            match std::process::Command::new("systemctl")
                .args(["--user", "start", unit])
                .output()
            {
                Ok(output) if output.status.success() => {
                    return Ok(json!({
                        "name": agent_name,
                        "handed_back": true,
                        "writers_ended": terminated,
                        "tab_closed": tab_closed,
                        "wrapper": {"systemd_unit": unit},
                        "next": format!("agentctl tail {agent_name} --follow"),
                    }));
                }
                Ok(output) => {
                    unit_error = Some(String::from_utf8_lossy(&output.stderr).trim().to_owned());
                }
                Err(error) => unit_error = Some(error.to_string()),
            }
        }
        // No unit, or it would not start: run the wrapper detached instead.
        let pid = self.launch_wrapper(agent_name)?;
        Ok(json!({
            "name": agent_name,
            "handed_back": true,
            "writers_ended": terminated,
            "tab_closed": tab_closed,
            "wrapper": {"pid": pid, "systemd_unit_failed": unit.map(|unit| json!({"unit": unit, "error": unit_error}))},
            "next": format!("agentctl tail {agent_name} --follow"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_arguments_drop_the_permission_prompt_choice() {
        let given: Vec<String> = [
            "--model",
            "m",
            "--permission-prompts",
            "none",
            "--settings=f",
            "--permission-prompts=none",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(
            interactive_arguments(&given),
            vec!["--model", "m", "--settings=f"]
        );
    }

    #[test]
    fn a_takeover_needs_handback_once_it_ended_or_its_holder_is_gone() {
        assert!(takeover_needs_handback(&json!({"state": "ended"})));
        assert!(takeover_needs_handback(&json!({"state": "failed"})));
        assert!(!takeover_needs_handback(&json!({"state": "stopping"})));
        let own = agent_queue::identity::observe(u64::from(std::process::id())).unwrap();
        let live = json!({"state": "interactive", "holder_identity": own.identity});
        assert!(!takeover_needs_handback(&live));
    }
}

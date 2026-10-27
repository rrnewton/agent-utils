//! Headless agent records (`adapter: harness-wrapper`, `#565 headless-coordinator` M2).
//!
//! A headless agent has no pane. Its wrapper (`agentctl run-headless NAME`) is the only drainer
//! of its queue, so every command here only enqueues, asks the wrapper through its control
//! socket, or reads the event log. The record keeps the harness session in
//! `extra.headless.session`, and the harness arguments in `arguments`.

use super::*;
use crate::headless::{self as hl, claude, control as hcontrol, events, wrapper};

mod takeover;
pub use takeover::TakeoverOptions;
use takeover::{takeover_marker, takeover_needs_handback};

/// Schema of the `headless` block in a headless agent's record.
const SPEC_SCHEMA: &str = "agentctl-headless/v1";
/// How long `stop` waits for a running wrapper to finish and release its lock.
const STOP_WAIT: Duration = Duration::from_secs(150);
/// How long a control request may take.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the chat bridge waits for a wrapper to answer a wake. The input is already queued,
/// and a wrapper that does not answer takes it at its next queue poll.
const CHAT_WAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// What `start --mode headless` registers.
#[derive(Clone, Debug, Default)]
pub struct HeadlessStart {
    /// Harness to run; only `claude` in this build.
    pub harness: String,
    /// Model passed to the harness as `--model`.
    pub model: Option<String>,
    /// Further harness arguments, such as `--settings FILE` or `--permission-mode dontAsk`.
    pub arguments: Vec<String>,
    /// Session to resume (a UUID begun in the same working directory); a new one otherwise.
    pub session: Option<String>,
    /// Start the wrapper as a detached process; `false` only registers (for a systemd unit).
    pub launch: bool,
    /// First input, queued before the wrapper starts.
    pub brief: Option<String>,
    /// Record the calling agent as creator and manager.
    pub record_edges: bool,
}

impl AgentRecord {
    /// A headless agent driven by `agentctl run-headless`.
    pub(super) fn is_headless(&self) -> bool {
        self.adapter == hl::ADAPTER
    }

    /// The harness session of a headless record.
    fn headless_session(&self) -> Result<String> {
        self.extra
            .get("headless")
            .filter(|spec| spec["schema"] == SPEC_SCHEMA)
            .and_then(|spec| spec["session"].as_str())
            .filter(|session| claude::is_uuid(session))
            .map(str::to_owned)
            .ok_or_else(|| {
                fail(format!(
                    "headless agent {:?} has no valid session in its record",
                    self.name
                ))
            })
    }
}

/// Records each harness process the wrapper starts as the record's `harness_identity`, the
/// process whose descendants act for the agent: agentctl's caller-ancestry checks and
/// agent-teams sender proof find a headless coordinator through it.
struct RecordIdentity<'m, 'a, A: ManagedApi + ?Sized> {
    manager: &'m ManagedAgents<'a, A>,
    name: String,
    token: String,
}

impl<A: ManagedApi + ?Sized> wrapper::Hooks for RecordIdentity<'_, '_, A> {
    fn harness_spawned(&self, pid: u32) -> std::result::Result<(), String> {
        let observed = agent_queue::identity::observe(u64::from(pid))
            .map_err(|error| format!("observe harness process {pid}: {error}"))?;
        // A stop holds the lifecycle lock while it waits for this wrapper; never wait long.
        let _lock = self
            .manager
            .lock_with_runtime(
                &self.name,
                &agent::GraceRuntime::new(Duration::from_secs(5)),
            )
            .map_err(|error| format!("lock the record: {error}"))?;
        let mut record = self
            .manager
            .load(&self.name)
            .map_err(|error| error.to_string())?;
        if record.token != self.token {
            return Err("the record was replaced; not recording the harness".to_owned());
        }
        record.harness_identity = Some(observed.identity);
        self.manager
            .save(&record)
            .map_err(|error| error.to_string())
    }
}

/// How long to wait for queue items to settle, and whether they did.
struct Settled {
    delivered: Vec<String>,
    pending: Vec<String>,
    quarantined: Vec<String>,
    cancelled: Vec<String>,
}

fn settle(queue_root: &Path, ids: &[String], wait: Duration) -> Settled {
    let deadline = Instant::now() + wait;
    loop {
        let mut settled = Settled {
            delivered: Vec::new(),
            pending: Vec::new(),
            quarantined: Vec::new(),
            cancelled: Vec::new(),
        };
        for id in ids {
            // A rename in progress shows an item in two phases for an instant; it is unsettled.
            match agent_queue::queue::message_state(queue_root, id) {
                Ok(Some(agent_queue::queue::MessageState::Processed)) => {
                    settled.delivered.push(id.clone())
                }
                Ok(Some(agent_queue::queue::MessageState::Failed)) => {
                    settled.quarantined.push(id.clone())
                }
                Ok(Some(agent_queue::queue::MessageState::Cancelled)) => {
                    settled.cancelled.push(id.clone())
                }
                Ok(None) => {}
                Ok(Some(_)) | Err(_) => settled.pending.push(id.clone()),
            }
        }
        if settled.pending.is_empty() || Instant::now() >= deadline {
            return settled;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn queue_result(message_id: String, settled: Settled, blocked: Option<String>) -> QueueResult {
    let outcome = if !settled.quarantined.is_empty() {
        agent::QueueOutcome::PossiblySubmitted
    } else if !settled.pending.is_empty() {
        agent::QueueOutcome::Pending
    } else {
        agent::QueueOutcome::Delivered
    };
    let blocked = blocked.or_else(|| {
        (!settled.pending.is_empty()).then(|| {
            "the headless agent has not started every input yet; it is between tool calls or running a long one (see agentctl tail)".to_owned()
        })
    });
    QueueResult {
        message_id,
        delivered: settled.delivered,
        quarantined: settled.quarantined,
        pending: settled.pending,
        cancelled: settled.cancelled,
        blocked,
        outcome,
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Whether a registered agent is headless.
    pub fn is_headless(&self, agent_name: &str) -> Result<bool> {
        Ok(self.load(agent_name)?.is_headless())
    }

    /// Register a headless agent and, unless `options.launch` is false, start its wrapper as a
    /// detached process.
    pub fn start_headless(
        &self,
        agent_name: &str,
        cwd: &Path,
        options: HeadlessStart,
    ) -> Result<Value> {
        if !hl::HARNESSES.contains(&options.harness.as_str()) {
            return Err(fail(format!(
                "headless mode runs {} in this build, not {:?}",
                hl::HARNESSES.join(", "),
                options.harness
            )));
        }
        claude::validate_arguments(&options.arguments).map_err(fail)?;
        let cwd = fs::canonicalize(cwd)
            .map_err(|error| fail(format!("working directory {}: {error}", cwd.display())))?;
        let session = match options.session {
            Some(session) if claude::is_uuid(&session) => session.to_ascii_lowercase(),
            Some(session) => {
                return Err(fail(format!(
                    "--resume for a headless Claude agent takes a session UUID, not {session:?}"
                )))
            }
            None => claude::random_uuid()
                .map_err(|error| fail(format!("make a session ID: {error}")))?,
        };
        let mut arguments = Vec::new();
        if let Some(model) = options.model.as_deref() {
            arguments.extend(["--model".to_owned(), model.to_owned()]);
        }
        arguments.extend(options.arguments.iter().cloned());
        let lock = self.lock(agent_name)?;
        let directory = self.directory(agent_name)?;
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(fail(format!(
                "agent {agent_name:?} already registered; stop it before reusing the name"
            )));
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&self.registry)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?;
        let mut extra = BTreeMap::new();
        extra.insert(
            "headless".to_owned(),
            json!({"schema": SPEC_SCHEMA, "harness": options.harness, "session": session}),
        );
        let mut record = AgentRecord {
            storage_directory: None,
            nested: None,
            adapter: hl::ADAPTER.to_owned(),
            mode: hl::MODE.to_owned(),
            backend: "none".to_owned(),
            paused: false,
            runtime_home: None,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: None,
            terminal_id: None,
            harness_identity: None,
            name_history: Vec::new(),
            anchor_rule: None,
            former_names: Vec::new(),
            native_session: None,
            profile: None,
            slot: None,
            slot_project: None,
            slot_isolation: None,
            reasoning_effort: None,
            environment_names: Vec::new(),
            agentcloud: None,
            launch_spec: None,
            extra,
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            harness: options.harness.clone(),
            cwd: cwd.display().to_string(),
            created_at: now.as_secs_f64(),
            schema: 1,
            lifecycle: "running".to_owned(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            session_agent: None,
            session_value: None,
            model: options.model.clone(),
            resume: None,
            arguments,
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
            goal_message_id: None,
        };
        self.creation_edges(&mut record, options.record_edges);
        self.save(&record)?;
        let queue_root = self.queue(agent_name)?;
        agent_queue::queue::prepare(&queue_root)?;
        let brief_id = match options.brief.as_deref().filter(|text| !text.is_empty()) {
            Some(text) => Some(self.enqueue_headless(
                &queue_root,
                text,
                None,
                "normal",
                "turn-boundary",
                None,
            )?),
            None => None,
        };
        drop(lock);
        let launched = if options.launch {
            Some(self.launch_wrapper(agent_name)?)
        } else {
            None
        };
        let mut result = self.status(agent_name)?;
        result["brief_message_id"] = json!(brief_id);
        result["wrapper_launched_pid"] = json!(launched);
        result["next"] = json!(if options.launch {
            format!("agentctl tail {agent_name} --follow")
        } else {
            format!(
                "register no detached wrapper: run `agentctl --registry {} run-headless {agent_name}` under a supervisor (agentctl install-units --headless {agent_name})",
                self.registry.display()
            )
        });
        Ok(result)
    }

    /// Start `agentctl run-headless NAME` in its own session, logging to the agent directory.
    fn launch_wrapper(&self, agent_name: &str) -> Result<u32> {
        let executable = std::env::current_exe()
            .map_err(|error| fail(format!("locate this agentctl: {error}")))?;
        let log_path = self.directory(agent_name)?.join("headless.log");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|error| fail(format!("open {}: {error}", log_path.display())))?;
        let log_err = log.try_clone().map_err(|error| fail(error.to_string()))?;
        let mut command = std::process::Command::new(executable);
        command
            .arg("--registry")
            .arg(&self.registry)
            .args(["run-headless", agent_name])
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err);
        // SAFETY: setsid is async-signal-safe and touches no memory of this process.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut command, || {
                libc::setsid();
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|error| fail(format!("start the headless wrapper: {error}")))?;
        let pid = child.id();
        // The wrapper runs on its own; reap it in the background if it exits while we live.
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        let socket = hcontrol::socket_path(&self.directory(agent_name)?);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if hcontrol::request(&socket, &json!({"op": "status"}), Duration::from_secs(2)).is_ok()
            {
                return Ok(pid);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(fail(format!(
            "the headless wrapper (pid {pid}) did not open its control socket within 10 s; see {}",
            log_path.display()
        )))
    }

    /// Run the wrapper for a headless agent in the foreground until it stops.
    pub fn run_headless(
        &self,
        agent_name: &str,
        claude_bin: Option<&Path>,
        tool_preview_lines: Option<usize>,
        terminate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<i32> {
        let record = self.load(agent_name)?;
        if !record.is_headless() {
            return Err(fail(format!(
                "agent {agent_name:?} is not a headless agent (adapter {:?})",
                record.adapter
            )));
        }
        if record.lifecycle != "running" {
            return Err(fail(format!(
                "headless agent {agent_name:?} is {}; nothing to run",
                record.lifecycle
            )));
        }
        if let Some(marker) = takeover_marker(&record) {
            // Exit 0, so a supervisor that restarts on failure does not loop.
            eprintln!(
                "agentctl run-headless: {agent_name} is taken over by an interactive session (tab {}); run `agentctl handback {agent_name}` to resume it headless",
                marker["tab"].as_str().unwrap_or("?")
            );
            return Ok(0);
        }
        let session = record.headless_session()?;
        let directory = self.directory(agent_name)?;
        let mut config =
            wrapper::WrapperConfig::new(agent_name, &directory, Path::new(&record.cwd), &session);
        if let Some(bin) = claude_bin {
            config.command = vec![bin.display().to_string()];
        }
        config.arguments = record.arguments.clone();
        if let Some(lines) = tool_preview_lines {
            config.tool_preview_lines = lines;
        }
        let hooks = RecordIdentity {
            manager: self,
            name: agent_name.to_owned(),
            token: record.token.clone(),
        };
        wrapper::run(config, terminate, &hooks)
    }

    fn enqueue_headless(
        &self,
        queue_root: &Path,
        text: &str,
        message_id: Option<&str>,
        priority: &str,
        deliver: &str,
        not_after: Option<u64>,
    ) -> Result<String> {
        let from = std::env::var("AGENTCTL_AGENT")
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "agentctl".to_owned());
        Ok(agent_queue::queue::enqueue_into(
            queue_root,
            agent_queue::queue::Lane::InboxV2,
            message_id,
            true,
            &agent_queue::fsio::Blocking,
            |identifier| {
                // A chat request prompt names its message's create time, as for a pane queue:
                // open it with the delay as of queuing, so the stored text stays printable.
                let mut item = json!({
                    "id": identifier,
                    "text": text,
                    "queued_at": agent_queue::queue::unix_seconds(),
                    "delivery_attempts": 0,
                    "from": from,
                    "priority": priority,
                    "deliver": deliver,
                    "not_after": not_after,
                });
                if let Some(queued) =
                    crate::prompt_time::queue_deferred(text, crate::prompt_time::now_millis())
                {
                    item["text"] = json!(queued.text);
                    item["sent_at"] = json!(queued.sent_at);
                    item["opening"] = json!(queued.opening);
                }
                item
            },
        )?)
    }

    /// Ask the running wrapper of `agent_name` to act: `wake`, `status`, `interrupt` or `stop`.
    pub fn headless_control(&self, agent_name: &str, op: &str) -> Result<Value> {
        let record = self.load(agent_name)?;
        if !record.is_headless() {
            return Err(fail(format!(
                "{op} applies to headless agents; {agent_name:?} has adapter {:?}",
                record.adapter
            )));
        }
        let socket = hcontrol::socket_path(&self.directory(agent_name)?);
        let answer = hcontrol::request(&socket, &json!({"op": op}), CONTROL_TIMEOUT)
            .map_err(|error| fail(format!("{agent_name}: {error}")))?;
        if answer["ok"] != true {
            return Err(fail(format!(
                "{agent_name}: the wrapper refused {op}: {}",
                answer["error"].as_str().unwrap_or("no reason given")
            )));
        }
        Ok(json!({"name": agent_name, "op": op, "answer": answer}))
    }

    fn wake(&self, agent_name: &str) -> Option<String> {
        self.wake_within(agent_name, CONTROL_TIMEOUT)
    }

    fn wake_within(&self, agent_name: &str, timeout: Duration) -> Option<String> {
        let socket = hcontrol::socket_path(&self.directory(agent_name).ok()?);
        match hcontrol::request(&socket, &json!({"op": "wake"}), timeout) {
            Ok(answer) if answer["ok"] == true => None,
            Ok(answer) => Some(format!(
                "the headless wrapper refused wake: {}",
                answer["error"].as_str().unwrap_or("no reason given")
            )),
            Err(error) => Some(format!(
                "{error}; the input stays queued until the wrapper starts"
            )),
        }
    }

    /// Queue one input for a headless agent, wake its wrapper, and wait up to `wait` for the
    /// model to start it. `urgent` delivers mid-turn, between tool calls.
    pub fn send_headless(
        &self,
        agent_name: &str,
        text: &str,
        message_id: Option<&str>,
        urgent: bool,
        not_after: Option<u64>,
        wait: Duration,
    ) -> Result<QueueResult> {
        let (queue_root, id) = {
            let _lock = self.lock(agent_name)?;
            let record = self.load(agent_name)?;
            self.enqueue_for(&record, text, message_id, urgent, not_after)?
        };
        let blocked = self.wake(agent_name);
        let wait = if blocked.is_some() {
            Duration::ZERO
        } else {
            wait
        };
        Ok(queue_result(
            id.clone(),
            settle(&queue_root, &[id], wait),
            blocked,
        ))
    }

    /// The headless form of a managed send by one of agentctl's own senders (watch-agents and
    /// restore notices, the chat bridge), which hold the agent's lifecycle lock: enqueue, wake,
    /// and wait briefly. `deliver_while_working` makes the input urgent, delivered mid-turn.
    pub(super) fn send_headless_record(
        &self,
        record: &AgentRecord,
        text: &str,
        message_id: Option<&str>,
        options: &DrainOptions,
    ) -> Result<QueueResult> {
        let (queue_root, id) = self.enqueue_for(
            record,
            text,
            message_id,
            options.deliver_while_working,
            options.not_after,
        )?;
        let blocked = self.wake(&record.name);
        // The caller holds the lifecycle lock that `stop` needs: never wait long under it.
        let wait = if blocked.is_some() {
            Duration::ZERO
        } else {
            options.ready_timeout.min(Duration::from_secs(10))
        };
        Ok(queue_result(
            id.clone(),
            settle(&queue_root, &[id], wait),
            blocked,
        ))
    }

    /// The chat bridge's form of a headless send (#594 headless-slack-trial): enqueue under the
    /// agent's lifecycle `lock`, release it, wake the wrapper briefly, and wait as a send does.
    /// The wake is made outside the lock, which the wrapper takes when it records a new harness.
    /// It succeeds only once the model has started the input; until then it fails as `Pending`,
    /// so the bridge keeps the request pending, retries it, and its stall alarm covers a wrapper
    /// that is down. An input set aside as possibly submitted fails as such.
    pub(super) fn send_headless_for_chat(
        &self,
        lock: File,
        record: &AgentRecord,
        text: &str,
        message_id: &str,
        options: &DrainOptions,
    ) -> Result<QueueResult> {
        let (queue_root, id) = self.enqueue_for(
            record,
            text,
            Some(message_id),
            options.deliver_while_working,
            options.not_after,
        )?;
        drop(lock);
        let blocked = self.wake_within(&record.name, CHAT_WAKE_TIMEOUT);
        let wait = if blocked.is_some() {
            Duration::ZERO
        } else {
            options.ready_timeout.min(Duration::from_secs(10))
        };
        let result = queue_result(
            id.clone(),
            settle(&queue_root, std::slice::from_ref(&id), wait),
            blocked,
        );
        let undelivered = |state: &str| agent::UndeliveredMessage {
            message: format!(
                "queued for headless agent {:?}, which has {state}: {}",
                record.name,
                result.blocked.as_deref().unwrap_or("see agentctl tail")
            ),
            message_id: id.clone(),
            artifact: queue_root.clone(),
        };
        match result.outcome {
            agent::QueueOutcome::Delivered => Ok(result),
            agent::QueueOutcome::PossiblySubmitted => Err(AgentError::PossiblySubmitted(
                undelivered("set it aside as possibly submitted"),
            )),
            _ => Err(AgentError::Pending(undelivered("not started it yet"))),
        }
    }

    /// The headless form of a drain by the chat bridge, called with the agent's lifecycle
    /// `lock`: the wrapper is the only drainer, so release the lock, wake the wrapper briefly, and
    /// report the inputs still waiting, without waiting for them.
    pub(super) fn wake_headless_record(
        &self,
        lock: File,
        record: &AgentRecord,
    ) -> Result<QueueResult> {
        if record.lifecycle != "running" {
            return Err(fail(format!(
                "agent {:?} is not a running headless agent",
                record.name
            )));
        }
        if record.paused {
            return Err(fail(format!("agent {:?} is paused", record.name)));
        }
        let queue_root = self.queue(&record.name)?;
        let ids = agent_queue::queue::pending_identifiers(&queue_root)?;
        drop(lock);
        let blocked = self.wake_within(&record.name, CHAT_WAKE_TIMEOUT);
        Ok(queue_result(
            String::new(),
            settle(&queue_root, &ids, Duration::ZERO),
            blocked,
        ))
    }

    /// A headless queue entry's state as the chat bridge reads it: an input written to the
    /// harness that the model has still to start (`inflight`) is on its way, as one in the inbox
    /// is, not possibly lost as a pane prompt the queue began to type would be.
    pub(super) fn headless_message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> Result<Option<agent::QueueMessageState>> {
        Ok(
            match agent::message_state(&self.queue(agent_name)?, message_id)? {
                Some(agent::QueueMessageState::Inflight) => Some(agent::QueueMessageState::Pending),
                state => state,
            },
        )
    }

    fn enqueue_for(
        &self,
        record: &AgentRecord,
        text: &str,
        message_id: Option<&str>,
        urgent: bool,
        not_after: Option<u64>,
    ) -> Result<(PathBuf, String)> {
        if !record.is_headless() || record.lifecycle != "running" {
            return Err(fail(format!(
                "agent {:?} is not a running headless agent",
                record.name
            )));
        }
        if record.paused {
            return Err(fail(format!("agent {:?} is paused", record.name)));
        }
        let (priority, deliver) = if urgent {
            ("urgent", "now")
        } else {
            ("normal", "turn-boundary")
        };
        let queue_root = self.queue(&record.name)?;
        let id =
            self.enqueue_headless(&queue_root, text, message_id, priority, deliver, not_after)?;
        Ok((queue_root, id))
    }

    /// Wake a headless agent's wrapper and wait for the inputs pending now to settle.
    pub fn drain_headless(&self, agent_name: &str, options: &DrainOptions) -> Result<QueueResult> {
        let record = self.load(agent_name)?;
        if !record.is_headless() {
            return Err(fail(format!("agent {agent_name:?} is not headless")));
        }
        let queue_root = self.queue(agent_name)?;
        let mut ids = agent_queue::queue::pending_identifiers(&queue_root)?;
        ids.extend(agent_queue::queue::identifiers_if_directory(
            &queue_root.join(agent_queue::queue::INFLIGHT),
        )?);
        if let Some(limit) = options.max_items {
            ids.truncate(limit);
        }
        let blocked = self.wake(agent_name);
        let wait = if blocked.is_some() {
            Duration::ZERO
        } else {
            options.deadline.unwrap_or(options.ready_timeout)
        };
        Ok(queue_result(
            String::new(),
            settle(&queue_root, &ids, wait),
            blocked,
        ))
    }

    /// Queue `/compact` for a headless agent; it runs when the current turn ends.
    pub fn compact_headless(&self, agent_name: &str) -> Result<Value> {
        let queue_root = self.queue(agent_name)?;
        if !self.load(agent_name)?.is_headless() {
            return Err(fail(format!("agent {agent_name:?} is not headless")));
        }
        let id = self.enqueue_headless(
            &queue_root,
            "/compact",
            None,
            "normal",
            "turn-boundary",
            None,
        )?;
        let blocked = self.wake(agent_name);
        Ok(
            json!({"name": agent_name, "message_id": id, "blocked": blocked,
                  "next": format!("agentctl tail {agent_name} --types compact.")}),
        )
    }

    /// The status row of a headless agent: the record, the wrapper's answer, the queue, and
    /// the last event.
    pub(super) fn headless_status_record(&self, record: &AgentRecord) -> Result<Value> {
        let mut result = self.record_row(record)?;
        let directory = self.directory(&record.name)?;
        let socket = hcontrol::socket_path(&directory);
        let (agent_status, wrapper) = if record.lifecycle != "running" {
            ("stopped", Value::Null)
        } else if let Some(marker) = takeover_marker(record) {
            result["takeover"] = marker.clone();
            result["takeover_needs_handback"] = json!(takeover_needs_handback(marker));
            ("taken-over", Value::Null)
        } else {
            match hcontrol::request(&socket, &json!({"op": "status"}), Duration::from_secs(10)) {
                Ok(answer) => {
                    let status = match answer["status"].as_str() {
                        Some("busy") => "working",
                        Some("idle") => "idle",
                        Some("starting") => "starting",
                        Some("stopped") => "stopped",
                        _ => "unknown",
                    };
                    (status, answer)
                }
                Err(hcontrol::ClientError::NotRunning(detail)) => {
                    result["probe_error"] =
                        json!(format!("the headless wrapper is not running ({detail})"));
                    ("dead", Value::Null)
                }
                Err(error) => {
                    result["probe_error"] = json!(error.to_string());
                    ("unreachable", Value::Null)
                }
            }
        };
        result["agent_status"] = json!(agent_status);
        result["wrapper"] = wrapper;
        let queue_root = self.queue(&record.name)?;
        result["queue_state"] = json!({
            "pending": agent_queue::queue::pending_identifiers(&queue_root).unwrap_or_default().len(),
            "inflight": agent_queue::queue::identifiers_if_directory(&queue_root.join(agent_queue::queue::INFLIGHT)).unwrap_or_default().len(),
            "failed": agent_queue::queue::identifiers_if_directory(&queue_root.join(agent_queue::queue::FAILED)).unwrap_or_default().len(),
        });
        let events_dir = events::directory(&directory);
        result["event_log"] = json!(events_dir);
        result["last_event"] = events::read_index(&events_dir).map_or(
            Value::Null,
            |index| json!({"seq": index["last_seq"], "updated_at": index["updated_at"]}),
        );
        result["session"] = json!(record.headless_session().ok());
        Ok(result)
    }

    /// Stop a headless agent's wrapper cleanly, wait for it to exit, and archive the record
    /// with its queue and event log.
    pub(super) fn retire_headless(
        &self,
        record: AgentRecord,
        options: &StopOptions,
    ) -> Result<Value> {
        let name = record.name.clone();
        if options.retire_dead_adoption
            || options.recover_legacy_adoption
            || options.expected_record_sha256.is_some()
            || options.skip_cloud_halt
        {
            return Err(fail(
                "adoption recovery and --skip-cloud-halt do not apply to headless agents",
            ));
        }
        if options
            .expected_token
            .as_deref()
            .is_some_and(|expected| expected != record.token)
        {
            return Err(fail(format!(
                "agent {name:?} was replaced before this operation"
            )));
        }
        if let Some(marker) = takeover_marker(&record) {
            return Err(fail(format!(
                "agent {name:?} is taken over by an interactive session (tab {}); run agentctl handback {name} first, or exit that session",
                marker["tab"].as_str().unwrap_or("?")
            )));
        }
        let directory = self.directory(&name)?;
        let socket = hcontrol::socket_path(&directory);
        let asked = hcontrol::request(&socket, &json!({"op": "stop"}), CONTROL_TIMEOUT).is_ok();
        let deadline = Instant::now() + STOP_WAIT;
        let lock = loop {
            if let Some(lock) = wrapper::try_lock(&directory)? {
                break lock;
            }
            if Instant::now() >= deadline {
                return Err(super::turn_runner::unavailable(format!(
                    "the headless wrapper of {name:?} still runs after {} s; the record is unchanged",
                    STOP_WAIT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let pinned = self.pinned_agent_directory(&name)?;
        let initial = self.managed_record_snapshot(&pinned, &record.token)?;
        let (_archive, destination) = self.archive_destination(&initial.record)?;
        let mut stopped = initial.record;
        stopped.lifecycle = "stopped".to_owned();
        let mut stopped_bytes = serde_json::to_vec_pretty(&stopped)
            .map_err(|error| fail(format!("cannot serialize agent record: {error}")))?;
        stopped_bytes.push(b'\n');
        if stopped_bytes.len() > MAX_AGENT_RECORD_BYTES {
            return Err(fail(format!(
                "refusing stopped agent record larger than {MAX_AGENT_RECORD_BYTES} bytes"
            )));
        }
        atomic_replace_bytes(&pinned, "agent.json", &stopped_bytes)
            .map_err(|error| *error.error)?;
        self.publish_pinned_directory(&pinned, &destination, &stopped_bytes)?;
        drop(lock);
        let unit = format!("agentctl-headless-{name}.service");
        Ok(json!({
            "name": name,
            "archive": destination,
            "wrapper_asked_to_stop": asked,
            "session": stopped.headless_session().ok(),
            "pane_closed": false,
            "tab_closed": false,
            "note": format!("if a systemd unit ran the wrapper, disable it so it does not start a wrapper for a missing record: systemctl --user disable {unit}"),
        }))
    }
}

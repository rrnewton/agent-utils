//! Discoverable command-line interface for persistent named agents.
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;

use crate::agent::{AgentError, DrainOptions, QueueOutcome};
use crate::client::HerdrClient;
use crate::subagents::{environment_entries, AdoptOptions, ManagedAgents, StartOptions};

const MAX_CHAT_PUBLISH_BYTES: usize = 30_000;

#[derive(Parser)]
#[command(
    name = "agentctl",
    version,
    about = "Manage persistent coding agents that you can message and inspect",
    long_about = "Manage named coding-agent sessions with durable prompt delivery and direct human access.\nThe Rust implementation controls interactive Codex/Claude sessions through Herdr and can run a durable event-driven chat bridge through installed subscription plugins.",
    after_help = "Examples:\n  agentctl quickstart\n  agentctl start reviewer --cwd .\n  agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex\n  agentctl send reviewer 'Review the current changes'\n  agentctl chat status --bridge-state ~/.local/state/agentctl/chat\n  agentctl pause reviewer\n  agentctl attach reviewer\n  agentctl resume reviewer\n  agentctl stop reviewer"
)]
struct Cli {
    /// Registry directory containing agent records and durable queues
    #[arg(
        long,
        visible_alias = "state",
        global = true,
        default_value = ".agentctl",
        value_name = "DIR"
    )]
    registry: PathBuf,
    /// Herdr executable name on PATH or an explicit path (interactive adapter only)
    #[arg(long, global = true, default_value = "herdr", value_name = "PATH")]
    herdr_bin: PathBuf,
    /// Print the installed operator reference and exit
    #[arg(long)]
    userguide: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show installed runtime adapters and optional services as JSON
    #[command(after_help = "Example: agentctl capabilities")]
    Capabilities,
    /// Launch an interactive agent in its own Herdr tab
    #[command(
        after_help = "Example: agentctl start reviewer --cwd . --harness codex --brief 'Review the changes'"
    )]
    Start(Start),
    /// Register an existing Herdr agent without taking ownership of its runtime
    #[command(
        after_help = "Example: agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex"
    )]
    Adopt(Adopt),
    /// Stop an owned runtime, or safely unregister an adopted one, and archive state
    #[command(after_help = "Example: agentctl stop reviewer")]
    Stop(Named),
    /// List registered agents with live status or an explicit probe error
    #[command(after_help = "Example: agentctl list --registry .agentctl")]
    List,
    /// Show durable metadata, queue state, and the live runtime probe
    #[command(after_help = "Example: agentctl status reviewer")]
    Status(Named),
    /// Persist and deliver a prompt when the agent is ready
    #[command(
        after_help = "Examples:\n  agentctl send reviewer 'Review the diff'\n  agentctl send reviewer --file task.txt --message-id review-1"
    )]
    Send(Send),
    /// Deliver queued prompts that are known not to have been submitted
    #[command(after_help = "Example: agentctl drain reviewer --ready-timeout 60")]
    Drain(Drain),
    /// Read visible terminal output and save a bounded snapshot
    #[command(after_help = "Example: agentctl read reviewer --lines 100")]
    Read(Read),
    /// Wait until the agent is ready for input; this does not prove goal completion
    #[command(after_help = "Example: agentctl wait reviewer --timeout 60")]
    Wait(Wait),
    /// Inspect a goal or set an ongoing objective (native /goal for Codex)
    #[command(
        after_help = "Examples:\n  agentctl goal reviewer\n  agentctl goal reviewer 'Complete the review and report blockers'"
    )]
    Goal(Goal),
    /// Bind an explicitly known native conversation ID to a verified agent pane
    #[command(after_help = "Example: agentctl bind-session reviewer THREAD_ID")]
    BindSession(BindSession),
    /// Verify and focus the agent pane for human interaction; does not pause automation
    #[command(after_help = "Example: agentctl pause reviewer && agentctl attach reviewer")]
    Attach(Named),
    /// Pause automation input without suspending the harness or its running work
    #[command(after_help = "Example: agentctl pause reviewer")]
    Pause(Named),
    /// Re-enable automation input; queued messages still require drain or send
    #[command(after_help = "Example: agentctl resume reviewer && agentctl drain reviewer")]
    Resume(Named),
    /// Initialize, inspect, recover, or run a durable chat bridge
    #[command(
        after_help = "Example: agentctl chat status --bridge-state ~/.local/state/agentctl/chat"
    )]
    Chat(Chat),
    /// Print a short, runnable introduction to starting and controlling a worker
    Quickstart,
    /// Print the operator reference, including dependencies and delivery guarantees
    Userguide,
}

#[derive(Args)]
struct Named {
    /// Registered agent name (1-32 lowercase letters, digits, and hyphens)
    name: String,
}

#[derive(Args)]
struct Delivery {
    /// Seconds to wait for a submit-safe state (0 probes immediately)
    #[arg(long, default_value = "900", value_parser = seconds)]
    ready_timeout: f64,
    /// Positive seconds to wait for evidence that a submitted prompt started work
    #[arg(long, default_value = "30", value_parser = positive_seconds)]
    working_timeout: f64,
    /// Maximum known-safe submission attempts per message
    #[arg(long, default_value = "3", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    max_attempts: u64,
}
impl Delivery {
    fn options(&self) -> DrainOptions {
        DrainOptions {
            ready_timeout: Duration::from_secs_f64(self.ready_timeout),
            working_timeout: Duration::from_secs_f64(self.working_timeout),
            max_attempts: self.max_attempts,
        }
    }
}

#[derive(Args)]
struct Start {
    #[command(flatten)]
    agent: Named,
    /// Working directory for the new harness
    #[arg(long, default_value = ".", value_name = "DIR")]
    cwd: PathBuf,
    /// Execution mode; headless workers require an installation with the worker extension
    #[arg(long, default_value = "interactive", value_parser = ["interactive", "headless"])]
    mode: String,
    /// Terminal host; interactive Rust agents require Herdr
    #[arg(long, default_value = "herdr", value_parser = ["herdr", "tmux"])]
    backend: String,
    /// Herdr harness kind; Codex and Claude have model/resume presets
    #[arg(long, default_value = "codex")]
    harness: String,
    /// Model identifier passed unchanged to the Codex or Claude harness
    #[arg(long)]
    model: Option<String>,
    /// Existing native conversation ID to resume instead of starting a new one
    #[arg(long, value_name = "SESSION")]
    resume: Option<String>,
    /// Extra literal harness argument; repeat or use --harness-arg=--flag
    #[arg(long = "harness-arg", value_name = "ARG")]
    harness_args: Vec<String>,
    /// Set a literal KEY=VALUE variable in the created interactive Herdr tab; repeatable and omitted from status
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = environment_value)]
    environment: Vec<String>,
    /// Initial prompt to deliver after startup; conflicts with --file
    #[arg(long, conflicts_with = "file", value_name = "TEXT")]
    brief: Option<String>,
    /// UTF-8 file containing the initial prompt; conflicts with --brief
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Existing Herdr workspace ID (otherwise HERDR_WORKSPACE_ID or a subagents workspace)
    #[arg(long, value_name = "ID")]
    workspace_id: Option<String>,
    /// Seconds to wait for harness startup, greater than zero and at most 300
    #[arg(long, default_value = "30", value_parser = startup_seconds)]
    startup_timeout: f64,
    #[command(flatten)]
    delivery: Delivery,
}

#[derive(Args)]
struct Adopt {
    #[command(flatten)]
    agent: Named,
    /// Exact live Herdr pane containing the agent (required)
    #[arg(long, value_name = "ID")]
    pane: String,
    /// Expected live Herdr workspace label; a mismatch is refused (required)
    #[arg(long, value_name = "LABEL")]
    workspace: String,
    /// Expected live agent working directory; compared canonically (required)
    #[arg(long, value_name = "DIR")]
    cwd: PathBuf,
    /// Expected live Herdr harness kind, such as codex or claude (required)
    #[arg(long, value_name = "KIND")]
    harness: String,
    /// Stable native conversation ID already reported by this exact pane
    #[arg(long, value_name = "ID")]
    session: Option<String>,
}

#[derive(Args)]
struct Prompt {
    /// Prompt text; use --file for a multiline or large prompt
    #[arg(conflicts_with = "file")]
    text: Option<String>,
    /// UTF-8 file containing prompt text
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
}
impl Prompt {
    fn read(&self) -> Result<Option<String>, String> {
        let text = self.file.as_ref().map_or_else(
            || Ok(self.text.clone()),
            |path| {
                fs::read_to_string(path)
                    .map(Some)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))
            },
        )?;
        if text.as_deref().is_some_and(|text| text.trim().is_empty()) {
            return Err("instruction must not be empty".to_owned());
        }
        Ok(text)
    }

    fn read_bounded(&self, maximum: usize, label: &str) -> Result<Option<String>, String> {
        let text = match self.file.as_ref() {
            Some(path) => {
                let file = fs::File::open(path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                let limit = u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1);
                let mut bytes = Vec::with_capacity(maximum.saturating_add(1));
                file.take(limit)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                if bytes.len() > maximum {
                    return Err(format!("{label} exceeds {maximum} UTF-8 bytes"));
                }
                Some(
                    String::from_utf8(bytes)
                        .map_err(|_| format!("{} does not contain valid UTF-8", path.display()))?,
                )
            }
            None => self.text.clone(),
        };
        if text.as_deref().is_some_and(|text| text.trim().is_empty()) {
            return Err(format!("{label} must not be empty"));
        }
        if text.as_ref().is_some_and(|text| text.len() > maximum) {
            return Err(format!("{label} exceeds {maximum} UTF-8 bytes"));
        }
        Ok(text)
    }
}

#[derive(Args)]
struct Send {
    #[command(flatten)]
    agent: Named,
    #[command(flatten)]
    prompt: Prompt,
    /// Stable caller-supplied ID; an ID already present in any delivery state is refused
    #[arg(long, value_name = "ID")]
    message_id: Option<String>,
    /// Model override for a headless turn (requires an installation with the worker extension)
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct Drain {
    #[command(flatten)]
    agent: Named,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct Read {
    #[command(flatten)]
    agent: Named,
    /// Maximum terminal lines to read
    #[arg(long, default_value = "500", value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
    lines: u32,
    /// Output boundary; last and since_turn require headless transcripts
    #[arg(long, default_value = "tail", value_parser = ["tail", "all", "last", "since_turn"])]
    output: String,
    /// First included headless turn; requires the worker extension and --output since_turn
    #[arg(long, value_name = "NUMBER")]
    since_turn: Option<u64>,
}
#[derive(Args)]
struct Wait {
    #[command(flatten)]
    agent: Named,
    /// Seconds to wait for readiness (0 probes immediately)
    #[arg(long, default_value = "900", value_parser = seconds)]
    timeout: f64,
}
#[derive(Args)]
struct Goal {
    #[command(flatten)]
    agent: Named,
    #[command(flatten)]
    prompt: Prompt,
    /// JSON argv array for a native Codex goal-reader command
    #[arg(long, value_name = "JSON", value_parser = command_json)]
    goal_command_json: Option<GoalCommand>,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct BindSession {
    #[command(flatten)]
    agent: Named,
    /// Native conversation ID verified by the operator
    session: String,
    /// JSON argv array for a native Codex goal-reader command
    #[arg(long, value_name = "JSON", value_parser = command_json)]
    goal_command_json: Option<GoalCommand>,
}

#[derive(Args)]
struct Chat {
    #[command(subcommand)]
    command: ChatCommand,
}

#[derive(Subcommand)]
enum ChatCommand {
    /// Print the shortest safe setup for the event-driven chat bridge
    Quickstart,
    /// Print the chat bridge configuration and operations guide
    Userguide,
    /// Validate plugin, target, helper, and config authority, then create state
    Init(ChatInit),
    /// Inspect durable bridge state without contacting Herdr or a provider
    Status(ChatState),
    /// Publish one explicit operator root message through the configured helper
    Publish(ChatPublish),
    /// Run one bounded delivery, terminal-capture, and outbound recovery pass
    Tick(ChatOperate),
    /// Run event-driven provider and Herdr subscriptions until SIGINT or SIGTERM
    Run(ChatOperate),
    /// Stop accepting fenced replies for one exact retained request
    Close(ChatClose),
}

#[derive(Args)]
struct ChatState {
    /// Private durable bridge state directory
    #[arg(long, value_name = "DIR")]
    bridge_state: PathBuf,
}

#[derive(Args)]
struct ChatInit {
    #[command(flatten)]
    state: ChatState,
    /// Private owner-only JSON bridge configuration (maximum 1 MiB)
    #[arg(long, value_name = "FILE")]
    config: PathBuf,
}

#[derive(Args)]
struct ChatPublish {
    #[command(flatten)]
    state: ChatState,
    /// Exact configured provider channel or space authority
    #[arg(long, value_name = "CHANNEL")]
    channel_id: String,
    /// Caller-owned lowercase RFC 4122 version-4 idempotency UUID
    #[arg(long, value_name = "UUID", value_parser = operation_uuid)]
    request_id: String,
    /// Message text; use --file for multiline input
    #[arg(required_unless_present = "file", conflicts_with = "file")]
    text: Option<String>,
    /// UTF-8 file containing the message, read with a 30,000-byte bound
    #[arg(long, value_name = "PATH", required_unless_present = "text")]
    file: Option<PathBuf>,
}

impl ChatPublish {
    fn read_message(&self) -> Result<String, String> {
        Prompt {
            text: self.text.clone(),
            file: self.file.clone(),
        }
        .read_bounded(MAX_CHAT_PUBLISH_BYTES, "chat publish message")?
        .ok_or_else(|| "chat publish requires message text or --file".to_owned())
    }
}

#[derive(Args)]
struct ChatOperate {
    #[command(flatten)]
    state: ChatState,
    /// Seconds to wait for coordinator readiness; zero keeps daemon passes nonblocking
    #[arg(long, default_value = "0", value_parser = chat_delivery_seconds)]
    ready_timeout: f64,
    /// Positive seconds to wait for evidence a delivered prompt started work
    #[arg(long, default_value = "5", value_parser = positive_chat_delivery_seconds)]
    working_timeout: f64,
    /// Maximum safe terminal-injection attempts for one request
    #[arg(long, default_value = "1", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    max_attempts: u64,
    /// Seconds between disk-backed recovery snapshots; provider intake remains event-driven
    #[arg(long, default_value = "300", value_parser = positive_seconds)]
    reconcile_interval: f64,
}

impl ChatOperate {
    fn options(&self) -> crate::chat_service::ServiceOptions {
        crate::chat_service::ServiceOptions {
            delivery: DrainOptions {
                ready_timeout: Duration::from_secs_f64(self.ready_timeout),
                working_timeout: Duration::from_secs_f64(self.working_timeout),
                max_attempts: self.max_attempts,
            },
            reconciliation_interval: Duration::from_secs_f64(self.reconcile_interval),
        }
    }
}

#[derive(Args)]
struct ChatClose {
    #[command(flatten)]
    state: ChatState,
    /// Exact 64-character lowercase hexadecimal request key
    #[arg(long)]
    request: String,
}

fn seconds(value: &str) -> Result<f64, String> {
    let value: f64 = value.parse().map_err(|_| "expected a number of seconds")?;
    if !value.is_finite() || !(0.0..=31_536_000.0).contains(&value) {
        return Err("seconds must be finite and between 0 and 31536000".to_owned());
    }
    Ok(value)
}
fn positive_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value == 0.0 {
        return Err("seconds must be greater than zero".to_owned());
    }
    Ok(value)
}
fn startup_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value <= 0.0 || value > 300.0 {
        return Err("startup seconds must be greater than zero and at most 300".to_owned());
    }
    Ok(value)
}
fn chat_delivery_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value > 30.0 {
        return Err("chat delivery seconds must be at most 30".to_owned());
    }
    Ok(value)
}
fn positive_chat_delivery_seconds(value: &str) -> Result<f64, String> {
    let value = chat_delivery_seconds(value)?;
    if value == 0.0 {
        return Err("chat delivery seconds must be greater than zero".to_owned());
    }
    Ok(value)
}
fn environment_value(value: &str) -> Result<String, String> {
    environment_entries(&[value.to_owned()])
        .map(|entries| entries.into_iter().next().expect("one validated entry"))
        .map_err(|error| error.to_string())
}
fn operation_uuid(value: &str) -> Result<String, String> {
    crate::chat_runtime::validate_operation_uuid(value)
        .map(|()| value.to_owned())
        .map_err(|error| error.to_string())
}
#[derive(Clone, Debug)]
struct GoalCommand(Vec<String>);
fn command_json(value: &str) -> Result<GoalCommand, String> {
    let command: Vec<String> = serde_json::from_str(value)
        .map_err(|error| format!("expected a JSON argv array: {error}"))?;
    if command.is_empty()
        || command
            .iter()
            .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err("command must contain nonempty arguments without NUL".to_owned());
    }
    Ok(GoalCommand(command))
}

/// Run the canonical CLI with arguments excluding the executable name.
pub fn main<I: IntoIterator<Item = OsString>>(arguments: I) -> i32 {
    let args =
        match Cli::try_parse_from(std::iter::once(OsString::from("agentctl")).chain(arguments)) {
            Ok(args) => args,
            Err(error) => {
                let code = error.exit_code();
                let _ = error.print();
                return code;
            }
        };
    match run(args) {
        Ok(code) => code,
        Err(Failure::Agent(error)) => {
            if let Some(message) = error.undelivered() {
                if let Err(output) = write_json(
                    &json!({"outcome": error.outcome().map(QueueOutcome::as_str), "message_id": message.message_id, "artifact": message.artifact, "error": error.to_string(), "safe_to_retry": error.safe_to_retry()}),
                ) {
                    eprintln!("agentctl: {output}");
                    return 1;
                }
            } else {
                eprintln!("agentctl: {error}");
            }
            error.exit_code()
        }
        Err(Failure::Usage(error)) => {
            eprintln!("agentctl: {error}");
            2
        }
        Err(Failure::Output(error)) => {
            eprintln!("agentctl: cannot write output: {error}");
            1
        }
        Err(Failure::Chat(error)) => {
            eprintln!("agentctl: {error}");
            1
        }
    }
}

enum Failure {
    Agent(AgentError),
    Usage(String),
    Output(io::Error),
    Chat(crate::chat_service::ChatServiceError),
}
impl From<AgentError> for Failure {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}
fn write_json(value: &impl Serialize) -> io::Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")
}

fn plugin_discovery_required(command: &Commands) -> bool {
    matches!(command, Commands::Capabilities)
}

fn run(args: Cli) -> Result<i32, Failure> {
    use clap::CommandFactory;
    if args.userguide {
        print!("{}", crate::USER_GUIDE);
        return Ok(0);
    }
    let Some(command) = args.command else {
        Cli::command().print_help().map_err(Failure::Output)?;
        println!();
        return Ok(0);
    };
    // Directory and manifest I/O is lazy. Session-control commands never inspect the user plugin
    // home; capability inspection and an actual plugin runtime startup own discovery.
    let plugin_inventory = plugin_discovery_required(&command).then(crate::plugins::discover);
    match command {
        Commands::Capabilities => {
            write_json(&capabilities_document(
                args.registry,
                plugin_inventory.expect("capabilities requires plugin discovery"),
            ))
            .map_err(Failure::Output)?;
            return Ok(0);
        }
        Commands::Quickstart => {
            print!("{}", crate::QUICKSTART);
            return Ok(0);
        }
        Commands::Userguide => {
            print!("{}", crate::USER_GUIDE);
            return Ok(0);
        }
        Commands::Chat(value) => {
            return run_chat(args.registry, args.herdr_bin, value);
        }
        _ => {}
    }
    let client =
        HerdrClient::with_executable("direct", &args.herdr_bin).map_err(AgentError::from)?;
    let manager = ManagedAgents::new(&client, &args.registry)?;
    let mut result = match command {
        Commands::Start(value) => {
            if value.mode != "interactive" || value.backend != "herdr" {
                return Err(Failure::Usage("Rust agentctl supports interactive Herdr sessions; use agentctl with the worker extension for headless workers".to_owned()));
            }
            let brief = match value.file {
                Some(path) => Some(
                    fs::read_to_string(path).map_err(|error| Failure::Usage(error.to_string()))?,
                ),
                None => value.brief,
            };
            manager.start(
                &value.agent.name,
                &value.cwd,
                StartOptions {
                    workspace_id: value.workspace_id,
                    harness: value.harness,
                    model: value.model,
                    resume: value.resume,
                    harness_args: value.harness_args,
                    environment: value.environment,
                    brief,
                    startup_timeout: Duration::from_secs_f64(value.startup_timeout),
                    delivery: value.delivery.options(),
                },
            )?
        }
        Commands::Adopt(value) => manager.adopt(
            &value.agent.name,
            AdoptOptions {
                pane_id: value.pane,
                expected_workspace: value.workspace,
                cwd: value.cwd,
                harness: value.harness,
                session: value.session,
            },
        )?,
        Commands::Stop(value) => manager.stop(&value.name)?,
        Commands::List => json!(manager.list()?),
        Commands::Status(value) => manager.status(&value.name)?,
        Commands::Send(value) => {
            if value.model.is_some() {
                return Err(Failure::Usage(
                    "--model on send requires a headless worker and an installation with the worker extension"
                        .to_owned(),
                ));
            }
            let text = value
                .prompt
                .read()
                .map_err(Failure::Usage)?
                .ok_or_else(|| Failure::Usage("send requires prompt text or --file".to_owned()))?;
            json!(manager.send_identified(
                &value.agent.name,
                &text,
                value.delivery.options(),
                value.message_id.as_deref()
            )?)
        }
        Commands::Drain(value) => {
            let result = manager.drain(&value.agent.name, value.delivery.options())?;
            write_json(&result).map_err(Failure::Output)?;
            return Ok(if result.blocked.is_some() {
                75
            } else if result.quarantined.is_empty() {
                0
            } else {
                76
            });
        }
        Commands::Read(value) => {
            if !matches!(value.output.as_str(), "tail" | "all") || value.since_turn.is_some() {
                return Err(Failure::Usage("interactive agents expose terminal snapshots; last/since_turn require headless transcripts".to_owned()));
            }
            print!("{}", manager.read(&value.agent.name, value.lines as usize)?);
            return Ok(0);
        }
        Commands::Wait(value) => {
            manager.wait(&value.agent.name, Duration::from_secs_f64(value.timeout))?
        }
        Commands::Goal(value) => manager.goal(
            &value.agent.name,
            value.prompt.read().map_err(Failure::Usage)?.as_deref(),
            value.delivery.options(),
            value
                .goal_command_json
                .as_ref()
                .map(|command| command.0.as_slice()),
        )?,
        Commands::BindSession(value) => manager.bind_session(
            &value.agent.name,
            &value.session,
            value
                .goal_command_json
                .as_ref()
                .map(|command| command.0.as_slice()),
        )?,
        Commands::Attach(value) => manager.attach(&value.name)?,
        Commands::Pause(value) => manager.pause(&value.name, true)?,
        Commands::Resume(value) => manager.pause(&value.name, false)?,
        Commands::Capabilities | Commands::Quickstart | Commands::Userguide | Commands::Chat(_) => {
            unreachable!()
        }
    };
    add_capabilities(&mut result);
    write_json(&result).map_err(Failure::Output)?;
    Ok(0)
}

fn run_chat(registry: PathBuf, herdr_bin: PathBuf, chat: Chat) -> Result<i32, Failure> {
    match chat.command {
        ChatCommand::Quickstart => {
            print!("{}", crate::CHAT_QUICKSTART);
            Ok(0)
        }
        ChatCommand::Userguide => {
            print!("{}", crate::CHAT_USER_GUIDE);
            Ok(0)
        }
        ChatCommand::Status(value) => {
            let result = crate::chat_service::status(&value.bridge_state).map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Publish(value) => {
            let body = value.read_message().map_err(Failure::Usage)?;
            let result = crate::chat_service::publish(
                &value.state.bridge_state,
                &value.channel_id,
                &value.request_id,
                &body,
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Close(value) => {
            let result = crate::chat_service::close(&value.state.bridge_state, &value.request)
                .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Init(value) => {
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            let manager = ManagedAgents::new(&client, &registry)?;
            let result =
                crate::chat_service::initialize(&value.state.bridge_state, &value.config, &manager)
                    .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Tick(value) => {
            let options = value.options();
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            let manager = ManagedAgents::new(&client, &registry)?;
            let result = crate::chat_service::tick(&value.state.bridge_state, &manager, options)
                .map_err(Failure::Chat)?;
            let code = if result.has_errors() { 75 } else { 0 };
            write_json(&result).map_err(Failure::Output)?;
            Ok(code)
        }
        ChatCommand::Run(value) => {
            let options = value.options();
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            let manager = ManagedAgents::new(&client, &registry)?;
            let result =
                crate::chat_service::run(&value.state.bridge_state, &client, &manager, options)
                    .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
    }
}

fn capabilities_document(
    registry: PathBuf,
    plugins: crate::plugins::PluginInventory,
) -> serde_json::Value {
    let not_live = crate::plugins::MaturityStatus::discovered(false);
    json!({
        "interactive": {
            "backends": ["herdr"],
            "harnesses": ["codex", "claude"]
        },
        "headless": null,
        "services": [{
            "name": "chat-bridge",
            "origin": "built_in",
            "status": crate::plugins::MaturityStatus::discovered(true)
        }],
        "registry": registry,
        "chat_subscriptions": {
            "core": {
                "origin": "built_in",
                "protocol_name": chat_subscription_plugin::PROTOCOL_NAME,
                "protocol_version": chat_subscription_plugin::PROTOCOL_VERSION,
                "max_frame_bytes": chat_subscription_plugin::MAX_FRAME_BYTES,
                "status": crate::plugins::MaturityStatus::discovered(true)
            },
            "reference_backends": [
                {
                    "name": "google-workspace-events",
                    "origin": "reference",
                    "status": not_live,
                    "available": ["design"],
                    "missing": [
                        "rust-provider",
                        "cloud-topic",
                        "pull-subscription",
                        "application-default-credentials",
                        "live-end-to-end-verification"
                    ]
                },
                {
                    "name": "discord-gateway",
                    "origin": "reference",
                    "status": crate::plugins::MaturityStatus::discovered(false),
                    "available": ["request-response-client"],
                    "missing": ["gateway-subscription", "live-end-to-end-verification"]
                }
            ],
            "plugins": plugins
        }
    })
}

fn add_capabilities(value: &mut serde_json::Value) {
    if let Some(values) = value.as_array_mut() {
        for value in values {
            add_capabilities(value);
        }
    } else if value.get("adapter").is_some() && value.get("name").is_some() {
        value["capabilities"] =
            if matches!(value["adapter"].as_str(), Some("herdr" | "herdr-foreign"))
                && value["mode"] == "interactive"
                && value["backend"] == "herdr"
            {
                json!([
                    "send",
                    "status",
                    "read",
                    "wait",
                    "stop",
                    "attach",
                    "pause",
                    "resume",
                    "terminal-snapshot",
                    "drain",
                    "goal",
                    "bind-session"
                ])
            } else {
                json!(["status"])
            };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    #[test]
    fn subcommands_have_local_help_and_registry_is_global() {
        Cli::command().debug_assert();
        for arguments in [
            ["agentctl", "--registry", "custom", "status", "worker"],
            ["agentctl", "status", "worker", "--state", "custom"],
        ] {
            assert_eq!(
                Cli::try_parse_from(arguments).unwrap().registry,
                PathBuf::from("custom")
            );
        }
        let error = Cli::try_parse_from(["agentctl", "send", "--help"])
            .err()
            .unwrap();
        let help = error.to_string();
        assert!(help.contains("--message-id"));
        assert!(help.contains("Seconds to wait"));
        assert!(!help.contains("--startup-timeout"));
        let error = Cli::try_parse_from(["agentctl", "adopt", "--help"])
            .err()
            .unwrap();
        let help = error.to_string();
        for required in ["--pane", "--workspace", "--cwd", "--harness", "--session"] {
            assert!(help.contains(required));
        }
        assert!(help.contains("without taking ownership"));
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "status",
            "--bridge-state",
            "/tmp/chat-state",
            "--registry",
            "/tmp/registry",
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["agentctl", "chat", "quickstart"]).is_ok());
        let error = Cli::try_parse_from(["agentctl", "chat", "run", "--help"])
            .err()
            .expect("chat run help");
        let help = error.to_string();
        for required in [
            "--bridge-state",
            "--ready-timeout",
            "--working-timeout",
            "--reconcile-interval",
        ] {
            assert!(help.contains(required));
        }
        let error = Cli::try_parse_from(["agentctl", "chat", "publish", "--help"])
            .err()
            .expect("chat publish help");
        let help = error.to_string();
        for required in ["--bridge-state", "--channel-id", "--request-id", "--file"] {
            assert!(help.contains(required));
        }
    }

    #[test]
    fn chat_publish_parses_exactly_one_bounded_input_shape_and_lowercase_uuid() {
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        let parsed = Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "hello",
        ])
        .expect("positional publish text");
        let Some(Commands::Chat(Chat {
            command: ChatCommand::Publish(publish),
        })) = parsed.command
        else {
            panic!("expected publish command");
        };
        assert_eq!(publish.channel_id, "spaces/example");
        assert_eq!(publish.request_id, uuid);
        assert_eq!(publish.text.as_deref(), Some("hello"));
        assert!(publish.file.is_none());

        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "--file",
            "message.txt",
        ])
        .is_ok());
        for invalid in [
            "123E4567-e89b-42d3-a456-426614174000",
            "123e4567-e89b-12d3-a456-426614174000",
            "not-a-uuid",
        ] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "publish",
                "--bridge-state",
                "/tmp/chat-state",
                "--channel-id",
                "spaces/example",
                "--request-id",
                invalid,
                "hello",
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "hello",
            "--file",
            "message.txt",
        ])
        .is_err());
    }
    #[test]
    fn invalid_timeouts_and_ambiguous_prompts_are_rejected() {
        for value in ["NaN", "inf", "-1", "31536001"] {
            assert!(seconds(value).is_err());
        }
        assert!(startup_seconds("0").is_err());
        assert!(positive_seconds("0").is_err());
        for option in ["--ready-timeout", "--working-timeout"] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "run",
                "--bridge-state",
                "/tmp/chat-state",
                option,
                "30.000001",
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "goal",
            "worker",
            "--goal-command-json",
            "[\"codex\",\"app-server\"]"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "agentctl", "send", "worker", "prompt", "--file", "task.txt"
        ])
        .is_err());
    }

    #[test]
    fn chat_capabilities_separate_code_installation_from_runtime_evidence() {
        let plugins = crate::plugins::PluginInventory {
            home: Some(PathBuf::from("/tmp/fixture-home")),
            discovered: vec![crate::plugins::DiscoveredPlugin {
                name: "fixture-chat".to_owned(),
                capability: "chat-subscription.fixture".to_owned(),
                executable: "backend".to_owned(),
                protocol_name: chat_subscription_plugin::PROTOCOL_NAME,
                protocol_min: 1,
                protocol_max: 1,
                origin: "discovered",
                status: crate::plugins::MaturityStatus::discovered(true),
                executable_identity: None,
            }],
            refused: vec![crate::plugins::RefusedPlugin {
                entry: "old-chat".to_owned(),
                code: "incompatible_protocol",
                detail: "fixture refusal".to_owned(),
            }],
        };
        let document = capabilities_document(PathBuf::from("registry"), plugins);
        assert_eq!(document["chat_subscriptions"]["core"]["origin"], "built_in");
        assert_eq!(
            document["chat_subscriptions"]["core"]["protocol_name"],
            "agentctl-chat-subscription"
        );
        let discovered = &document["chat_subscriptions"]["plugins"]["discovered"][0];
        assert_eq!(discovered["origin"], "discovered");
        assert_eq!(discovered["status"]["implemented"], true);
        assert_eq!(discovered["status"]["configured"], false);
        assert_eq!(discovered["status"]["connected"], false);
        assert_eq!(discovered["status"]["live_verified"], false);
        assert_eq!(
            document["chat_subscriptions"]["plugins"]["refused"][0]["code"],
            "incompatible_protocol"
        );
        assert_eq!(
            document["chat_subscriptions"]["reference_backends"][0]["status"]["implemented"],
            false
        );
        assert_eq!(
            document["chat_subscriptions"]["reference_backends"][0]["available"],
            json!(["design"])
        );
        assert!(
            document["chat_subscriptions"]["reference_backends"][0]["missing"]
                .as_array()
                .expect("missing capability list")
                .contains(&json!("rust-provider"))
        );
    }

    #[test]
    fn adopted_interactive_records_advertise_the_full_named_interface() {
        let mut record = json!({
            "name": "foreign",
            "adapter": "herdr-foreign",
            "mode": "interactive",
            "backend": "herdr",
        });
        add_capabilities(&mut record);
        assert_eq!(
            record["capabilities"],
            json!([
                "send",
                "status",
                "read",
                "wait",
                "stop",
                "attach",
                "pause",
                "resume",
                "terminal-snapshot",
                "drain",
                "goal",
                "bind-session"
            ])
        );
    }
}
#[test]
fn ordinary_session_control_does_not_discover_plugins() {
    assert!(plugin_discovery_required(&Commands::Capabilities));
    assert!(!plugin_discovery_required(&Commands::List));
    assert!(!plugin_discovery_required(&Commands::Status(Named {
        name: "worker".to_owned(),
    })));
}

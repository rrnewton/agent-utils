//! Durable coordinator inbox: notices about workers, coalesced and delivered as one batch.
//!
//! A coordinator that supervises several long-running workers needs to hear when one of them
//! stops, gets stuck, or reports something, without reading every worker's output on a timer and
//! without several writers typing into its session at once. This module keeps those notices in a
//! file-per-notice queue under the registry, applies the coalescing rules below, and renders the
//! queue as one prioritized text block.
//!
//! Layout under `<registry>/.inbox/<coordinator>/`:
//!
//! * `live/<created-ms>-<id>.json` — one queued notice per file.
//! * `claimed/<batch>.json` — a batch handed to a delivery adapter whose outcome is not yet known.
//!   A later `deliver` re-sends it with the same idempotency key before taking new notices.
//! * `delivered/<batch>.json` — delivered batches, newest [`DELIVERED_KEEP`] kept.
//! * `.lock` — held for every mutation; `.deliver.lock` — held for a whole delivery.
//!
//! Coalescing: a worker has at most one live *state* notice (`blocked`, `exited`, `idle`,
//! `still-idle`, `progress`); a newer one replaces it and widens its transcript range. Posting
//! `working` withdraws the live state notice, because the worker resumed on its own. `message`
//! notices are never coalesced; `--id` makes a repeated post a no-op.
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand, ValueEnum};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Most notices one coordinator's queue holds; further posts are refused as busy.
pub(crate) const DEFAULT_MAX_LIVE: usize = 200;
/// Default byte budget for one rendered batch.
pub(crate) const DEFAULT_RENDER_BYTES: usize = 3000;
/// Largest notice text accepted, in bytes.
pub(crate) const MAX_TEXT_BYTES: usize = 4000;
/// Bytes of one notice's text shown in a rendered batch before it is cut.
pub(crate) const RENDER_TEXT_BYTES: usize = 600;
/// Seconds after which an undelivered priority-3 notice expires.
pub(crate) const DEFAULT_STALE_SECONDS: u64 = 86_400;
/// Delivered batch records kept for inspection.
pub(crate) const DELIVERED_KEEP: usize = 200;
/// Largest text `agentcloudctl notify` is given, in bytes.
const MAX_NOTIFY_BYTES: usize = 100_000;
/// Busy: the queue is full or another delivery is running.
const EXIT_BUSY: i32 = crate::error::EXIT_BUSY;
/// The delivery adapter failed; the batch stays claimed for a retry with the same key.
const EXIT_UNAVAILABLE: i32 = 69;

/// What happened to the worker, or what it said.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NoticeKind {
    /// Waiting on a permission dialog, a question, or other outside input.
    Blocked,
    /// The worker's session or pane ended.
    Exited,
    /// The worker finished a turn and is waiting for input.
    Idle,
    /// An explicit message from the worker; never coalesced.
    Message,
    /// A reminder that an already-reported idle worker is still idle.
    StillIdle,
    /// New output while the worker keeps working.
    Progress,
    /// The worker resumed; withdraws its live state notice and stores nothing.
    Working,
}

impl NoticeKind {
    fn priority(self) -> u8 {
        match self {
            Self::Blocked | Self::Exited => 1,
            Self::Idle | Self::Message | Self::Working => 2,
            Self::StillIdle | Self::Progress => 3,
        }
    }

    fn is_state(self) -> bool {
        !matches!(self, Self::Message | Self::Working)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Exited => "exited",
            Self::Idle => "idle",
            Self::Message => "message",
            Self::StillIdle => "still-idle",
            Self::Progress => "progress",
            Self::Working => "working",
        }
    }
}

/// A byte range of a worker transcript that the notice covers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Cursor {
    path: String,
    start: u64,
    end: u64,
}

/// One queued notice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Notice {
    schema: u32,
    id: String,
    created_unix_ms: u64,
    updated_unix_ms: u64,
    kind: NoticeKind,
    priority: u8,
    agent: String,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<Cursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default)]
    replaced: u32,
}

/// A batch handed to an adapter.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Batch {
    schema: u32,
    id: String,
    coordinator: String,
    created_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivered_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<String>,
    attempts: u32,
    text: String,
    notices: Vec<Notice>,
}

/// Failure with the process exit code the CLI returns.
#[derive(Debug)]
pub(crate) struct InboxError {
    code: i32,
    message: String,
}

impl InboxError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    fn io(context: &str, error: &io::Error) -> Self {
        Self {
            code: 1,
            message: format!("{context}: {error}"),
        }
    }

    fn busy(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_BUSY,
            message: message.into(),
        }
    }

    /// Exit code for this failure.
    pub(crate) fn exit_code(&self) -> i32 {
        self.code
    }
}

impl std::fmt::Display for InboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

type Result<T> = std::result::Result<T, InboxError>;

/// `agentctl inbox` arguments.
#[derive(Args)]
pub(crate) struct InboxArgs {
    #[command(subcommand)]
    command: InboxCommand,
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Print a short introduction to the coordinator inbox
    Quickstart,
    /// Print the coordinator inbox reference: layout, coalescing, ordering, and delivery
    Userguide,
    /// Queue a notice about a worker for a coordinator
    #[command(
        after_help = "Examples:\n  agentctl inbox post --to coord --from kvm --kind idle --text 'Pushed 3f2a9c1; tests green'\n  agentctl inbox post --to coord --from kvm --kind message --id kvm-report-7 --text-file report.txt\n  agentctl inbox post --to coord --from kvm --kind working\n\nA worker keeps at most one live state notice (blocked, exited, idle, still-idle, progress): a\nnewer one replaces it. --kind working withdraws it and stores nothing. message notices are never\ncoalesced, and a repeated --id is a no-op that prints the existing notice id."
    )]
    Post(Post),
    /// List queued notices as JSON, in delivery order
    #[command(after_help = "Example: agentctl inbox list --to coord")]
    List(ListArgs),
    /// Print the next batch exactly as delivery would send it, without claiming it
    #[command(after_help = "Example: agentctl inbox render --to coord --max-bytes 2000")]
    Render(RenderArgs),
    /// Claim the next batch, hand it to an adapter, and record the outcome
    #[command(
        after_help = "Examples:\n  agentctl inbox deliver --to coord --via print\n  agentctl inbox deliver --to coord --via agentcloud-notify --session SESSION_ID\n\nprint writes the batch to stdout and counts that as delivered. agentcloud-notify runs\n`agentcloudctl notify --mode cli-script` with the idempotency key agentctl-inbox-<coordinator>-<batch>.\nIf that fails the batch stays claimed and the next deliver re-sends it with the same key, so the\nsession receives it once. An empty queue prints nothing and exits 0."
    )]
    Deliver(DeliverArgs),
}

#[derive(Args)]
struct Target {
    /// Coordinator whose inbox this is (1-64 lowercase letters, digits, dots, underscores, hyphens)
    #[arg(long = "to", value_name = "COORDINATOR")]
    coordinator: String,
}

#[derive(Args)]
struct Post {
    #[command(flatten)]
    target: Target,
    /// Worker the notice is about (same character rule as --to)
    #[arg(long = "from", value_name = "AGENT")]
    agent: String,
    /// What happened: blocked and exited are priority 1; idle and message 2; still-idle and progress 3
    #[arg(long, value_enum)]
    kind: NoticeKind,
    /// Notice text (up to 4000 bytes); give this or --text-file, except with --kind working
    #[arg(long, conflicts_with = "text_file")]
    text: Option<String>,
    /// Read the notice text from this file (up to 4000 bytes)
    #[arg(long, value_name = "PATH")]
    text_file: Option<PathBuf>,
    /// Idempotency key for a message notice; posting the same key again changes nothing
    #[arg(long, value_name = "KEY")]
    id: Option<String>,
    /// Transcript range covered, as PATH:START:END byte offsets (START <= END)
    #[arg(long, value_name = "PATH:START:END")]
    cursor: Option<String>,
    /// Most live notices the queue may hold before posts are refused with exit 75
    #[arg(long, default_value_t = DEFAULT_MAX_LIVE, value_name = "N")]
    max_live: usize,
}

#[derive(Args)]
struct ListArgs {
    #[command(flatten)]
    target: Target,
    /// Seconds after which an undelivered priority-3 notice is dropped as stale
    #[arg(long, default_value_t = DEFAULT_STALE_SECONDS, value_name = "SECONDS")]
    stale_after: u64,
}

#[derive(Args)]
struct RenderArgs {
    #[command(flatten)]
    target: Target,
    /// Byte budget for the batch; notices that do not fit stay queued (the first always fits)
    #[arg(long, default_value_t = DEFAULT_RENDER_BYTES, value_name = "BYTES")]
    max_bytes: usize,
    /// Seconds after which an undelivered priority-3 notice is dropped as stale
    #[arg(long, default_value_t = DEFAULT_STALE_SECONDS, value_name = "SECONDS")]
    stale_after: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Via {
    /// Write the batch to stdout; printing counts as delivery
    Print,
    /// Deliver into an agentcloud session with `agentcloudctl notify --mode cli-script`
    AgentcloudNotify,
}

#[derive(Args)]
struct DeliverArgs {
    #[command(flatten)]
    render: RenderArgs,
    /// Delivery adapter
    #[arg(long, value_enum)]
    via: Via,
    /// agentcloud session that receives the batch (agentcloud-notify only)
    #[arg(long, value_name = "ID")]
    session: Option<String>,
}

/// Run `agentctl inbox`.
pub(crate) fn run(registry: &Path, agentcloudctl: &Path, args: InboxArgs) -> Result<i32> {
    let now = now_ms()?;
    match args.command {
        InboxCommand::Quickstart => {
            print!("{}", crate::INBOX_QUICKSTART);
            Ok(0)
        }
        InboxCommand::Userguide => {
            print!("{}", crate::INBOX_USER_GUIDE);
            Ok(0)
        }
        InboxCommand::Post(post) => {
            let inbox = Inbox::open(registry, &post.target.coordinator)?;
            let text = match (&post.text, &post.text_file) {
                (Some(text), None) => text.clone(),
                (None, Some(path)) => fs::read_to_string(path).map_err(|error| {
                    InboxError::io(&format!("cannot read {}", path.display()), &error)
                })?,
                (None, None) if post.kind == NoticeKind::Working => String::new(),
                (None, None) => return Err(InboxError::usage("give --text or --text-file")),
                (Some(_), Some(_)) => unreachable!("clap rejects --text with --text-file"),
            };
            let cursor = post.cursor.as_deref().map(parse_cursor).transpose()?;
            let outcome = inbox.post(
                &PostRequest {
                    agent: &post.agent,
                    kind: post.kind,
                    text: &text,
                    key: post.id.as_deref(),
                    cursor,
                    max_live: post.max_live,
                },
                now,
            )?;
            print_json(&outcome)?;
            Ok(0)
        }
        InboxCommand::List(list) => {
            let inbox = Inbox::open(registry, &list.target.coordinator)?;
            let notices = inbox.live(now, list.stale_after)?;
            print_json(&notices)?;
            Ok(0)
        }
        InboxCommand::Render(render) => {
            let inbox = Inbox::open(registry, &render.target.coordinator)?;
            let notices = inbox.live(now, render.stale_after)?;
            let (text, _) = render_batch(
                &render.target.coordinator,
                "preview",
                &notices,
                render.max_bytes,
            );
            print!("{text}");
            Ok(0)
        }
        InboxCommand::Deliver(deliver) => {
            let coordinator = deliver.render.target.coordinator.clone();
            if deliver.via == Via::AgentcloudNotify
                && deliver.session.as_deref().is_none_or(str::is_empty)
            {
                return Err(InboxError::usage("--via agentcloud-notify needs --session"));
            }
            if deliver.via == Via::Print && deliver.session.is_some() {
                return Err(InboxError::usage(
                    "--session applies only to --via agentcloud-notify",
                ));
            }
            let inbox = Inbox::open(registry, &coordinator)?;
            let adapter = |batch: &Batch| -> Result<()> {
                match deliver.via {
                    Via::Print => {
                        let mut output = io::stdout().lock();
                        output
                            .write_all(batch.text.as_bytes())
                            .and_then(|()| output.flush())
                            .map_err(|error| InboxError::io("cannot write batch", &error))
                    }
                    Via::AgentcloudNotify => notify_agentcloud(
                        agentcloudctl,
                        deliver.session.as_deref().unwrap_or_default(),
                        &format!("agentctl-inbox-{coordinator}-{}", batch.id),
                        &batch.text,
                    ),
                }
            };
            let via = match deliver.via {
                Via::Print => "print",
                Via::AgentcloudNotify => "agentcloud-notify",
            };
            let delivered = inbox.deliver(
                now,
                deliver.render.max_bytes,
                deliver.render.stale_after,
                via,
                adapter,
            )?;
            if deliver.via != Via::Print {
                print_json(&serde_json::json!({ "delivered": delivered }))?;
            }
            Ok(0)
        }
    }
}

struct PostRequest<'a> {
    agent: &'a str,
    kind: NoticeKind,
    text: &'a str,
    key: Option<&'a str>,
    cursor: Option<Cursor>,
    max_live: usize,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct PostOutcome {
    /// Id of the stored notice, or of the existing one for a repeated key.
    id: Option<String>,
    /// `stored`, `replaced`, `duplicate`, `withdrawn`, or `nothing-to-withdraw`.
    outcome: &'static str,
    /// Id of the notice this one replaced or withdrew.
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<String>,
}

struct Inbox {
    root: PathBuf,
    coordinator: String,
}

impl Inbox {
    fn open(registry: &Path, coordinator: &str) -> Result<Self> {
        check_name("--to", coordinator)?;
        let root = registry.join(".inbox").join(coordinator);
        for directory in ["live", "claimed", "delivered"] {
            fs::create_dir_all(root.join(directory)).map_err(|error| {
                InboxError::io(&format!("cannot create {}", root.display()), &error)
            })?;
        }
        Ok(Self {
            root,
            coordinator: coordinator.to_owned(),
        })
    }

    fn lock(&self, name: &str, wait: bool) -> Result<fs::File> {
        let path = self.root.join(name);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| InboxError::io(&format!("cannot open {}", path.display()), &error))?;
        if wait {
            file.lock_exclusive().map_err(|error| {
                InboxError::io(&format!("cannot lock {}", path.display()), &error)
            })?;
        } else if file.try_lock_exclusive().is_err() {
            return Err(InboxError::busy(format!(
                "another delivery holds {}; retry later",
                path.display()
            )));
        }
        Ok(file)
    }

    fn read_dir_json<T: for<'de> Deserialize<'de>>(
        &self,
        directory: &str,
    ) -> Result<Vec<(PathBuf, T)>> {
        let path = self.root.join(directory);
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path)
            .map_err(|error| InboxError::io(&format!("cannot list {}", path.display()), &error))?
        {
            let entry = entry.map_err(|error| {
                InboxError::io(&format!("cannot list {}", path.display()), &error)
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                InboxError::io(&format!("cannot read {}", entry.path().display()), &error)
            })?;
            let value = serde_json::from_slice(&bytes).map_err(|error| InboxError {
                code: 1,
                message: format!("corrupt inbox record {}: {error}", entry.path().display()),
            })?;
            entries.push((entry.path(), value));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(entries)
    }

    fn write_json(&self, directory: &str, name: &str, value: &impl Serialize) -> Result<PathBuf> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| InboxError {
            code: 1,
            message: format!("cannot encode inbox record: {error}"),
        })?;
        let final_path = self.root.join(directory).join(name);
        let staging = self
            .root
            .join(directory)
            .join(format!(".{name}.{}.tmp", std::process::id()));
        let mut file = fs::File::create(&staging).map_err(|error| {
            InboxError::io(&format!("cannot write {}", staging.display()), &error)
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                InboxError::io(&format!("cannot write {}", staging.display()), &error)
            })?;
        fs::rename(&staging, &final_path).map_err(|error| {
            InboxError::io(&format!("cannot publish {}", final_path.display()), &error)
        })?;
        Ok(final_path)
    }

    /// Live notices in delivery order, after dropping stale priority-3 notices.
    fn live(&self, now: u64, stale_after: u64) -> Result<Vec<Notice>> {
        let _guard = self.lock(".lock", true)?;
        self.live_locked(now, stale_after)
    }

    fn live_locked(&self, now: u64, stale_after: u64) -> Result<Vec<Notice>> {
        let mut notices = Vec::new();
        for (path, notice) in self.read_dir_json::<Notice>("live")? {
            if notice.priority == 3
                && now.saturating_sub(notice.updated_unix_ms) > stale_after.saturating_mul(1000)
            {
                fs::remove_file(&path).map_err(|error| {
                    InboxError::io(&format!("cannot expire {}", path.display()), &error)
                })?;
                continue;
            }
            notices.push(notice);
        }
        notices.sort_by(|left, right| {
            (left.priority, left.created_unix_ms, &left.id).cmp(&(
                right.priority,
                right.created_unix_ms,
                &right.id,
            ))
        });
        Ok(notices)
    }

    fn post(&self, request: &PostRequest<'_>, now: u64) -> Result<PostOutcome> {
        check_name("--from", request.agent)?;
        if request.text.len() > MAX_TEXT_BYTES {
            return Err(InboxError::usage(format!(
                "notice text is {} bytes; the limit is {MAX_TEXT_BYTES}",
                request.text.len()
            )));
        }
        if request.key.is_some() && request.kind != NoticeKind::Message {
            return Err(InboxError::usage("--id applies only to --kind message"));
        }
        if request.kind == NoticeKind::Working
            && (!request.text.is_empty() || request.cursor.is_some())
        {
            return Err(InboxError::usage("--kind working takes no text or cursor"));
        }
        let _guard = self.lock(".lock", true)?;
        let live = self.read_dir_json::<Notice>("live")?;
        if let Some(key) = request.key {
            if let Some((_, existing)) = live
                .iter()
                .find(|(_, notice)| notice.key.as_deref() == Some(key))
            {
                return Ok(PostOutcome {
                    id: Some(existing.id.clone()),
                    outcome: "duplicate",
                    previous: None,
                });
            }
            if self.key_already_delivered(key)? {
                return Ok(PostOutcome {
                    id: None,
                    outcome: "duplicate",
                    previous: None,
                });
            }
        }
        let previous = request.kind.is_state() || request.kind == NoticeKind::Working;
        let previous = previous
            .then(|| {
                live.iter()
                    .find(|(_, notice)| notice.agent == request.agent && notice.kind.is_state())
            })
            .flatten();
        if request.kind == NoticeKind::Working {
            return Ok(match previous {
                Some((path, notice)) => {
                    fs::remove_file(path).map_err(|error| {
                        InboxError::io(&format!("cannot withdraw {}", path.display()), &error)
                    })?;
                    PostOutcome {
                        id: None,
                        outcome: "withdrawn",
                        previous: Some(notice.id.clone()),
                    }
                }
                None => PostOutcome {
                    id: None,
                    outcome: "nothing-to-withdraw",
                    previous: None,
                },
            });
        }
        if previous.is_none() && live.len() >= request.max_live {
            return Err(InboxError::busy(format!(
                "inbox for {} already holds {} notices (limit {})",
                self.coordinator,
                live.len(),
                request.max_live
            )));
        }
        let id = notice_id(request.agent, request.kind, request.key, now);
        let (created, cursor, replaced) = match previous {
            Some((_, old)) => (
                old.created_unix_ms,
                merge_cursor(old.cursor.as_ref(), request.cursor.as_ref()),
                old.replaced.saturating_add(1),
            ),
            None => (now, request.cursor.clone(), 0),
        };
        let notice = Notice {
            schema: 1,
            id: id.clone(),
            created_unix_ms: created,
            updated_unix_ms: now,
            kind: request.kind,
            priority: request.kind.priority(),
            agent: request.agent.to_owned(),
            text: request.text.to_owned(),
            cursor,
            key: request.key.map(str::to_owned),
            replaced,
        };
        self.write_json("live", &format!("{created:013}-{id}.json"), &notice)?;
        if let Some((path, old)) = previous {
            fs::remove_file(path).map_err(|error| {
                InboxError::io(&format!("cannot replace {}", path.display()), &error)
            })?;
            return Ok(PostOutcome {
                id: Some(id),
                outcome: "replaced",
                previous: Some(old.id.clone()),
            });
        }
        Ok(PostOutcome {
            id: Some(id),
            outcome: "stored",
            previous: None,
        })
    }

    fn key_already_delivered(&self, key: &str) -> Result<bool> {
        for directory in ["claimed", "delivered"] {
            for (_, batch) in self.read_dir_json::<Batch>(directory)? {
                if batch
                    .notices
                    .iter()
                    .any(|notice| notice.key.as_deref() == Some(key))
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Claim, deliver, and record one batch; returns the number of notices delivered.
    fn deliver(
        &self,
        now: u64,
        max_bytes: usize,
        stale_after: u64,
        via: &str,
        adapter: impl Fn(&Batch) -> Result<()>,
    ) -> Result<usize> {
        let _delivery = self.lock(".deliver.lock", false)?;
        let (path, mut batch) = {
            let _guard = self.lock(".lock", true)?;
            if let Some((path, batch)) = self.read_dir_json::<Batch>("claimed")?.into_iter().next()
            {
                (path, batch)
            } else {
                let notices = self.live_locked(now, stale_after)?;
                if notices.is_empty() {
                    return Ok(0);
                }
                let id = batch_id(&self.coordinator, now, &notices);
                let (text, taken) = render_batch(&self.coordinator, &id, &notices, max_bytes);
                let batch = Batch {
                    schema: 1,
                    id: id.clone(),
                    coordinator: self.coordinator.clone(),
                    created_unix_ms: now,
                    delivered_unix_ms: None,
                    via: Some(via.to_owned()),
                    attempts: 0,
                    text,
                    notices: notices.into_iter().take(taken).collect(),
                };
                let path = self.write_json("claimed", &format!("{now:013}-{id}.json"), &batch)?;
                let live = self.read_dir_json::<Notice>("live")?;
                for (live_path, notice) in live {
                    if batch.notices.iter().any(|taken| taken.id == notice.id) {
                        fs::remove_file(&live_path).map_err(|error| {
                            InboxError::io(&format!("cannot claim {}", live_path.display()), &error)
                        })?;
                    }
                }
                (path, batch)
            }
        };
        batch.attempts = batch.attempts.saturating_add(1);
        let attempt = adapter(&batch);
        let _guard = self.lock(".lock", true)?;
        if let Err(error) = attempt {
            self.write_json("claimed", &file_name(&path), &batch)?;
            return Err(InboxError {
                code: EXIT_UNAVAILABLE,
                message: format!(
                    "batch {} stays claimed after attempt {} and is re-sent with the same key next time: {error}",
                    batch.id, batch.attempts
                ),
            });
        }
        batch.delivered_unix_ms = Some(now_ms()?);
        self.write_json("delivered", &file_name(&path), &batch)?;
        fs::remove_file(&path).map_err(|error| {
            InboxError::io(&format!("cannot retire {}", path.display()), &error)
        })?;
        self.prune_delivered()?;
        Ok(batch.notices.len())
    }

    fn prune_delivered(&self) -> Result<()> {
        let delivered = self.read_dir_json::<serde_json::Value>("delivered")?;
        let excess = delivered.len().saturating_sub(DELIVERED_KEEP);
        for (path, _) in delivered.into_iter().take(excess) {
            fs::remove_file(&path).map_err(|error| {
                InboxError::io(&format!("cannot prune {}", path.display()), &error)
            })?;
        }
        Ok(())
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn check_name(flag: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(InboxError::usage(format!(
            "{flag} must be 1-64 lowercase letters, digits, dots, underscores or hyphens, starting with a letter or digit: {value:?}"
        )))
    }
}

fn parse_cursor(value: &str) -> Result<Cursor> {
    let invalid = || {
        InboxError::usage(format!(
            "--cursor must be PATH:START:END with START <= END: {value:?}"
        ))
    };
    let (rest, end) = value.rsplit_once(':').ok_or_else(invalid)?;
    let (path, start) = rest.rsplit_once(':').ok_or_else(invalid)?;
    let start: u64 = start.parse().map_err(|_| invalid())?;
    let end: u64 = end.parse().map_err(|_| invalid())?;
    if path.is_empty() || start > end {
        return Err(invalid());
    }
    Ok(Cursor {
        path: path.to_owned(),
        start,
        end,
    })
}

fn merge_cursor(old: Option<&Cursor>, new: Option<&Cursor>) -> Option<Cursor> {
    match (old, new) {
        (Some(old), Some(new)) if old.path == new.path => Some(Cursor {
            path: new.path.clone(),
            start: old.start.min(new.start),
            end: old.end.max(new.end),
        }),
        (_, Some(new)) => Some(new.clone()),
        (old, None) => old.cloned(),
    }
}

fn now_ms() -> Result<u64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| InboxError {
            code: 1,
            message: format!("system clock is before 1970: {error}"),
        })?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn short_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize()[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn notice_id(agent: &str, kind: NoticeKind, key: Option<&str>, now: u64) -> String {
    let nonce = format!("{}-{now}-{:?}", std::process::id(), SystemTime::now());
    short_hash(&[
        agent.as_bytes(),
        kind.label().as_bytes(),
        key.unwrap_or_default().as_bytes(),
        nonce.as_bytes(),
    ])
}

fn batch_id(coordinator: &str, now: u64, notices: &[Notice]) -> String {
    let ids = notices
        .iter()
        .map(|notice| notice.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}-{}",
        utc_compact(now),
        short_hash(&[coordinator.as_bytes(), ids.as_bytes()])
    )
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let day_of_year = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn utc_parts(ms: u64) -> (i64, u32, u32, u64, u64, u64) {
    let seconds = ms / 1000;
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    let rest = seconds % 86_400;
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

fn utc_compact(ms: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(ms);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

fn utc_clock(ms: u64) -> String {
    let (_, _, _, hour, minute, second) = utc_parts(ms);
    format!("{hour:02}:{minute:02}:{second:02}Z")
}

fn cut(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

fn render_notice(notice: &Notice) -> String {
    let mut block = format!(
        "[P{} {}] {} at {}",
        notice.priority,
        notice.kind.label(),
        notice.agent,
        utc_clock(notice.updated_unix_ms)
    );
    if notice.replaced > 0 {
        block.push_str(&format!(
            " (updated {}x since {})",
            notice.replaced,
            utc_clock(notice.created_unix_ms)
        ));
    }
    block.push('\n');
    let (text, truncated) = cut(notice.text.trim_end(), RENDER_TEXT_BYTES);
    for line in text.lines() {
        block.push_str("  ");
        block.push_str(line);
        block.push('\n');
    }
    if truncated {
        block.push_str(&format!(
            "  [cut at {RENDER_TEXT_BYTES} of {} bytes]\n",
            notice.text.len()
        ));
    }
    if let Some(cursor) = &notice.cursor {
        block.push_str(&format!(
            "  transcript: {} bytes {}..{}\n",
            cursor.path, cursor.start, cursor.end
        ));
    }
    block
}

/// Render notices (already in delivery order) into one batch; returns the text and how many
/// notices it includes. The first notice is always included; later ones only while the text
/// stays within `max_bytes`.
fn render_batch(
    coordinator: &str,
    batch: &str,
    notices: &[Notice],
    max_bytes: usize,
) -> (String, usize) {
    if notices.is_empty() {
        return (String::new(), 0);
    }
    let blocks = notices.iter().map(render_notice).collect::<Vec<_>>();
    let mut taken = 0;
    let mut body = String::new();
    for block in &blocks {
        if taken > 0 && body.len() + block.len() > max_bytes {
            break;
        }
        body.push_str(block);
        taken += 1;
    }
    let mut text = format!(
        "agentctl inbox for {coordinator}: {taken} of {} notices (batch {batch})\n",
        notices.len()
    );
    text.push_str(&body);
    if taken < notices.len() {
        text.push_str(&format!(
            "{} more stay queued: agentctl inbox list --to {coordinator}\n",
            notices.len() - taken
        ));
    }
    (text, taken)
}

fn notify_agentcloud(agentcloudctl: &Path, session: &str, key: &str, text: &str) -> Result<()> {
    if text.len() > MAX_NOTIFY_BYTES {
        return Err(InboxError::usage(format!(
            "batch is {} bytes; notify accepts {MAX_NOTIFY_BYTES}",
            text.len()
        )));
    }
    let output = Command::new(agentcloudctl)
        .args([
            "notify",
            "--session",
            session,
            "--mode",
            "cli-script",
            "--idempotency-key",
            key,
            "--text",
            text,
        ])
        .output()
        .map_err(|error| InboxError {
            code: EXIT_UNAVAILABLE,
            message: format!("cannot run {}: {error}", agentcloudctl.display()),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(InboxError {
            code: EXIT_UNAVAILABLE,
            message: format!(
                "{} notify exited {}: {}",
                agentcloudctl.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        })
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, value)
        .map_err(io::Error::from)
        .and_then(|()| output.write_all(b"\n"))
        .map_err(|error| InboxError::io("cannot write output", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-inbox-test-{}-{}",
            std::process::id(),
            short_hash(&[format!("{:?}", SystemTime::now()).as_bytes()])
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn post(inbox: &Inbox, agent: &str, kind: NoticeKind, text: &str, now: u64) -> PostOutcome {
        inbox
            .post(
                &PostRequest {
                    agent,
                    kind,
                    text,
                    key: None,
                    cursor: None,
                    max_live: DEFAULT_MAX_LIVE,
                },
                now,
            )
            .unwrap()
    }

    #[test]
    fn newer_state_replaces_older_and_widens_the_range() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        let first = PostRequest {
            agent: "kvm",
            kind: NoticeKind::Progress,
            text: "running tests",
            key: None,
            cursor: Some(parse_cursor("/t.jsonl:100:200").unwrap()),
            max_live: DEFAULT_MAX_LIVE,
        };
        assert_eq!(inbox.post(&first, 1_000).unwrap().outcome, "stored");
        let second = PostRequest {
            kind: NoticeKind::Idle,
            text: "done: 3f2a9c1",
            cursor: Some(parse_cursor("/t.jsonl:200:400").unwrap()),
            ..first
        };
        let outcome = inbox.post(&second, 2_000).unwrap();
        assert_eq!(outcome.outcome, "replaced");
        let live = inbox.live(2_000, DEFAULT_STALE_SECONDS).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].kind, NoticeKind::Idle);
        assert_eq!(live[0].created_unix_ms, 1_000);
        assert_eq!(live[0].replaced, 1);
        assert_eq!(
            live[0].cursor,
            Some(Cursor {
                path: "/t.jsonl".into(),
                start: 100,
                end: 400
            })
        );
    }

    #[test]
    fn working_withdraws_the_state_notice_but_not_messages() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "kvm", NoticeKind::Idle, "idle", 1);
        post(&inbox, "kvm", NoticeKind::Message, "note", 2);
        assert_eq!(
            post(&inbox, "kvm", NoticeKind::Working, "", 3).outcome,
            "withdrawn"
        );
        assert_eq!(
            post(&inbox, "kvm", NoticeKind::Working, "", 4).outcome,
            "nothing-to-withdraw"
        );
        let live = inbox.live(5, DEFAULT_STALE_SECONDS).unwrap();
        assert_eq!(
            live.iter().map(|notice| notice.kind).collect::<Vec<_>>(),
            vec![NoticeKind::Message]
        );
    }

    #[test]
    fn messages_are_never_coalesced_and_keys_are_idempotent_across_delivery() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        let keyed = |now| {
            inbox
                .post(
                    &PostRequest {
                        agent: "kvm",
                        kind: NoticeKind::Message,
                        text: "report",
                        key: Some("kvm-report-1"),
                        cursor: None,
                        max_live: DEFAULT_MAX_LIVE,
                    },
                    now,
                )
                .unwrap()
        };
        assert_eq!(keyed(1).outcome, "stored");
        assert_eq!(keyed(2).outcome, "duplicate");
        post(&inbox, "kvm", NoticeKind::Message, "second", 3);
        assert_eq!(inbox.live(4, DEFAULT_STALE_SECONDS).unwrap().len(), 2);
        assert_eq!(
            inbox
                .deliver(5, 10_000, DEFAULT_STALE_SECONDS, "print", |_| Ok(()))
                .unwrap(),
            2
        );
        assert_eq!(
            keyed(6).outcome,
            "duplicate",
            "a delivered key must not be re-queued"
        );
        assert!(inbox.live(7, DEFAULT_STALE_SECONDS).unwrap().is_empty());
    }

    #[test]
    fn order_is_priority_then_age() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "a", NoticeKind::Progress, "p3", 1);
        post(&inbox, "b", NoticeKind::Idle, "p2 older", 2);
        post(&inbox, "c", NoticeKind::Blocked, "p1", 3);
        post(&inbox, "d", NoticeKind::Message, "p2 newer", 4);
        let agents = inbox
            .live(5, DEFAULT_STALE_SECONDS)
            .unwrap()
            .into_iter()
            .map(|notice| notice.agent)
            .collect::<Vec<_>>();
        assert_eq!(agents, ["c", "b", "d", "a"]);
    }

    #[test]
    fn stale_priority_three_notices_expire_and_others_do_not() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "a", NoticeKind::StillIdle, "old reminder", 0);
        post(&inbox, "b", NoticeKind::Idle, "old idle", 0);
        let live = inbox.live(10_001, 10).unwrap();
        assert_eq!(
            live.iter()
                .map(|notice| notice.agent.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
    }

    #[test]
    fn a_full_queue_refuses_new_workers_but_still_accepts_replacements() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        let request = |agent, max_live| PostRequest {
            agent,
            kind: NoticeKind::Idle,
            text: "x",
            key: None,
            cursor: None,
            max_live,
        };
        inbox.post(&request("a", 1), 1).unwrap();
        let refused = inbox.post(&request("b", 1), 2).unwrap_err();
        assert_eq!(refused.exit_code(), EXIT_BUSY);
        assert_eq!(inbox.post(&request("a", 1), 3).unwrap().outcome, "replaced");
    }

    #[test]
    fn render_keeps_the_budget_but_always_includes_the_first_notice() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "a", NoticeKind::Blocked, &"x".repeat(900), 1);
        post(&inbox, "b", NoticeKind::Idle, "short", 2);
        let notices = inbox.live(3, DEFAULT_STALE_SECONDS).unwrap();
        let (text, taken) = render_batch("coord", "b1", &notices, 100);
        assert_eq!(taken, 1);
        assert!(text.starts_with(
            "agentctl inbox for coord: 1 of 2 notices (batch b1)\n[P1 blocked] a at "
        ));
        assert!(text.contains(&format!("[cut at {RENDER_TEXT_BYTES} of 900 bytes]")));
        assert!(text.ends_with("1 more stay queued: agentctl inbox list --to coord\n"));
        let (all, taken) = render_batch("coord", "b2", &notices, 10_000);
        assert_eq!(taken, 2);
        assert!(!all.contains("more stay queued"));
    }

    #[test]
    fn a_failed_delivery_is_resent_with_the_same_batch_before_new_notices() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "a", NoticeKind::Idle, "first", 1);
        let seen = std::cell::RefCell::new(Vec::new());
        let failed = inbox
            .deliver(
                2,
                10_000,
                DEFAULT_STALE_SECONDS,
                "agentcloud-notify",
                |batch| {
                    seen.borrow_mut().push(batch.id.clone());
                    Err(InboxError::busy("adapter down"))
                },
            )
            .unwrap_err();
        assert_eq!(failed.exit_code(), EXIT_UNAVAILABLE);
        post(
            &inbox,
            "b",
            NoticeKind::Idle,
            "arrived after the failure",
            3,
        );
        let count = Cell::new(0);
        let delivered = inbox
            .deliver(
                4,
                10_000,
                DEFAULT_STALE_SECONDS,
                "agentcloud-notify",
                |batch| {
                    seen.borrow_mut().push(batch.id.clone());
                    count.set(batch.notices.len());
                    assert_eq!(batch.attempts, 2);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!((delivered, count.get()), (1, 1));
        let ids = seen.into_inner();
        assert_eq!(
            ids[0], ids[1],
            "the retry must reuse the batch id, and so the idempotency key"
        );
        let remaining = inbox.live(5, DEFAULT_STALE_SECONDS).unwrap();
        assert_eq!(
            remaining
                .iter()
                .map(|notice| notice.agent.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
    }

    #[test]
    fn a_second_concurrent_delivery_is_refused_as_busy() {
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        post(&inbox, "a", NoticeKind::Idle, "x", 1);
        let other = Inbox::open(&registry, "coord").unwrap();
        let refused = inbox
            .deliver(2, 10_000, DEFAULT_STALE_SECONDS, "print", |_| {
                let error = other
                    .deliver(2, 10_000, DEFAULT_STALE_SECONDS, "print", |_| Ok(()))
                    .unwrap_err();
                assert_eq!(error.exit_code(), EXIT_BUSY);
                Ok(())
            })
            .unwrap();
        assert_eq!(refused, 1);
    }

    #[test]
    fn names_cursors_and_sizes_are_validated() {
        assert!(check_name("--to", "coord").is_ok());
        assert!(check_name("--to", "sub-worker_1.a").is_ok());
        for bad in ["", "Coord", "-x", "a/b", "a b", &"a".repeat(65)] {
            assert_eq!(
                check_name("--to", bad).unwrap_err().exit_code(),
                2,
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_cursor("/a:b:1:2").unwrap(),
            Cursor {
                path: "/a:b".into(),
                start: 1,
                end: 2
            }
        );
        for bad in ["/a:2:1", "/a:1", ":1:2", "/a:x:2"] {
            assert!(parse_cursor(bad).is_err(), "{bad:?}");
        }
        let registry = scratch();
        let inbox = Inbox::open(&registry, "coord").unwrap();
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        let request = PostRequest {
            agent: "a",
            kind: NoticeKind::Idle,
            text: &big,
            key: None,
            cursor: None,
            max_live: DEFAULT_MAX_LIVE,
        };
        assert_eq!(inbox.post(&request, 1).unwrap_err().exit_code(), 2);
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(utc_compact(0), "19700101T000000Z");
        assert_eq!(utc_compact(1_790_712_924_000), "20260929T201524Z");
        assert_eq!(utc_clock(1_709_210_096_000), "12:34:56Z");
        assert_eq!(utc_compact(951_782_400_000), "20000229T000000Z");
    }
}

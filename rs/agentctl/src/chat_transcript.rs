//! The coordinator's transcript as the source of its chat replies' text (#710
//! chat-transcript-extraction).
//!
//! `chat run` reads reply blocks from the coordinator's screen, where they are rendered: wrapped
//! at the terminal's width, with Markdown drawn as styles and tables as box drawing, and, in
//! Claude Code, collapsed to a summary line when a tool call follows them in the same turn. The
//! coordinator's harness also appends the conversation to a transcript of its own, where each
//! reply is exactly as the agent wrote it. [`CoordinatorTranscript`] finds that file from the
//! coordinator's registry record (its harness, process, working directory and conversation id),
//! follows it, and hands the capture the complete reply blocks it holds
//! ([`chat_runtime::TranscriptReplies`]). Everything about where each harness keeps its file and
//! how its lines read lives in the shared harness library; this module only decides when to look.
//!
//! The file is found again every [`RELOCATE_EVERY`], so a coordinator that restarts, clears its
//! conversation or is replaced is followed. A coordinator with no transcript, or one that cannot
//! be read, leaves every reply with its screen text, as before; each change in what is read is
//! logged once.
//!
//! A repoint changes the agent followed ([`TranscriptReplies::retarget`], #852
//! unknown-tui-bridge): the reader drops what it read of the previous agent and looks for the new
//! agent's transcript at once. It never goes on reading the previous agent's file, whose
//! following would hold the new agent's screen replies back as unpaired. A headless target's
//! transcript is its wrapper's typed event log (#1974):
//! the reader follows that log's `assistant.text` events exactly as it follows a harness
//! transcript's text, and only while no log is followed does a headless target reply solely
//! through `agentctl chat reply`. A target whose harness writes no transcript agentctl reads
//! (`--harness unknown`) is reported as replying through its screen and `agentctl chat reply`.
//!
//! A record can name no conversation and no process that names one, as an adopted coordinator's
//! can. Its transcript is then discovered (#783 chat-reply-capture-miss): first from the harness
//! process running in the coordinator's Herdr pane (the process whose environment names the
//! pane), then, for Claude Code, as the newest transcript in the coordinator's project directory
//! that mentions the reply ID of a request delivered to it. A conversation found through a live
//! process, for a record that names none, is offered for binding ([`ReplySource`]); one found only
//! by its content is read but never bound.

use std::collections::VecDeque;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_harness::{replies, Event, Follower, Hints, Located};
use serde::{Deserialize, Serialize};

use crate::chat_runtime::{TranscriptBlock, TranscriptReplies};

/// How often the transcript is looked for again.
pub(crate) const RELOCATE_EVERY: Duration = Duration::from_secs(30);
/// How far before its end a transcript is first read: replies written while the bridge was down
/// are found if they are this recent.
const FIRST_READ_BYTES: u64 = 32 * 1024 * 1024;
/// Most reply blocks kept, newest last.
const MAX_BLOCKS: usize = 512;
/// Most bytes of reply text kept.
const MAX_BLOCK_BYTES: usize = 4 * 1024 * 1024;
/// How long a capture waits for a transcript to catch up with a closing marker on screen.
const SETTLE_FOR: Duration = Duration::from_millis(1_000);
/// How often it looks while it waits.
const SETTLE_STEP: Duration = Duration::from_millis(200);
/// Most reply IDs remembered as waited for.
const MAX_WAITED: usize = 256;

/// Most transcripts in a project directory that discovery by content reads, newest first.
const DISCOVER_FILES: usize = 8;
/// How far before its end discovery by content reads each transcript.
const DISCOVER_TAIL_BYTES: u64 = 8 * 1024 * 1024;

/// What finds the coordinator now: its harness's name and what locates its transcript.
pub(crate) type Resolve = Box<dyn Fn() -> Option<(String, Hints)> + Send>;
/// What finds the coordinator's transcript when [`Resolve`] does not: see [`discover`].
pub(crate) type Discover = Box<dyn Fn() -> Option<Discovered> + Send>;
/// The event log directory of the current target when it is a headless agent, else `None`.
pub(crate) type HeadlessDir = Box<dyn Fn() -> Option<PathBuf> + Send>;
/// The reply IDs of the requests delivered to the coordinator and not answered yet.
pub(crate) type Expected = Box<dyn Fn() -> Vec<String> + Send>;
/// Where log lines go.
pub(crate) type Log = Box<dyn Fn(String) + Send>;

/// A transcript that discovery found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Discovered {
    /// The harness whose transcript it is.
    pub(crate) harness: String,
    /// The file, and the evidence that chose it.
    pub(crate) located: Located,
    /// The conversation id to bind into the record: only for a conversation a live process
    /// names, for a Claude Code record that names none.
    pub(crate) bind: Option<String>,
}

/// Where the bridge's reply text comes from now, as the last look for the transcript found it:
/// what the owner loop records for `chat health`, and the conversation it binds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReplySource {
    /// The transcript read, if any.
    pub(crate) path: Option<PathBuf>,
    /// Why that file, or why none.
    pub(crate) evidence: String,
    /// Whether the record named nothing that found it, so discovery did.
    pub(crate) discovered: bool,
    /// A conversation a live process names, for a Claude Code record that names none: the owner
    /// loop binds it into the record.
    pub(crate) unbound_session: Option<String>,
    /// Whether the target is an agentcloud session, whose replies the bridge reads from the
    /// session's own history (`agentcloudctl history`) rather than from a screen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) session_history: bool,
    /// Whether `path` is a headless target's event log directory, whose `assistant.text`
    /// events the bridge reads as its transcript (#1974).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) headless_log: bool,
}

/// The [`ReplySource`] the transcript reader shares with the owner loop: `None` until the first
/// look.
pub(crate) type SharedReplySource = Arc<Mutex<Option<ReplySource>>>;

/// Prompt and tool-call times the reader saw in the followed transcript, newest last, for
/// splitting a request's turn into thinking and tools (#897 chat-latency-stamps).
#[derive(Debug, Default)]
pub(crate) struct TurnMarks {
    /// Whether a transcript is followed now; while none is, the split is unavailable.
    pub(crate) following: bool,
    /// When prompts were recorded, Unix milliseconds.
    pub(crate) prompts: VecDeque<u64>,
    /// When tool calls were recorded, Unix milliseconds.
    pub(crate) tools: VecDeque<u64>,
}

/// Most times of each kind kept.
const MAX_TURN_MARKS: usize = 4_096;

impl TurnMarks {
    fn note(&mut self, event: &Event) {
        let at = event
            .at()
            .filter(|at| *at >= 0.0)
            .map(|at| (at * 1000.0) as u64);
        let (Some(at), list) = (
            at,
            match event {
                Event::Prompt { .. } => &mut self.prompts,
                Event::ToolCall { .. } => &mut self.tools,
                _ => return,
            },
        ) else {
            return;
        };
        // A tail read again repeats times; they stay in order either way.
        if list.back().is_some_and(|last| *last >= at) {
            return;
        }
        list.push_back(at);
        while list.len() > MAX_TURN_MARKS {
            list.pop_front();
        }
    }
}

/// The [`TurnMarks`] the transcript reader shares with the bridge state.
pub(crate) type SharedTurnMarks = Arc<Mutex<TurnMarks>>;

/// The agent a reader made by [`CoordinatorTranscript::for_agent`] follows: `None` while the
/// bridge serves no target agent, so the reader rests.
type SharedTarget = Arc<Mutex<Option<String>>>;

/// The evidence recorded for an agentcloud target, whose replies come from its session history.
pub(crate) const CLOUD_EVIDENCE: &str =
    "the target is an agentcloud session: the bridge reads its \
     replies from the session's own history (`agentcloudctl history`), and through `agentctl chat \
     reply`";

/// The evidence recorded while a headless target's event log is followed.
pub(crate) const HEADLESS_EVIDENCE: &str = "the target is headless: the bridge reads its replies \
     from its event log's assistant text, and through `agentctl chat reply`";

/// The evidence recorded while a headless target has no event log the bridge follows yet.
pub(crate) const HEADLESS_NO_LOG_EVIDENCE: &str = "the target is headless but no event log of it \
     is followed yet: its replies come only through `agentctl chat reply`";

/// The evidence recorded while the reader follows no target agent.
pub(crate) const RESTING_EVIDENCE: &str =
    "the bridge serves no target agent: no transcript is read";

/// Identity of the record/log a headless follower reads, by token and directory identity
/// rather than segment numbers (as `headless/tail.rs` checks, #1221): a replaced record under
/// the same name gets a new token and usually a fresh events directory, even when its new
/// generation numbers are lower than the old cursor (#1974 review).
#[derive(Clone, Debug, PartialEq, Eq)]
struct HeadlessIdentity {
    token: Option<String>,
    agent_dir: Option<(u64, u64)>,
    events_dir: Option<(u64, u64)>,
}

impl HeadlessIdentity {
    fn of(dir: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let identity = |path: &Path| {
            std::fs::metadata(path)
                .ok()
                .map(|meta| (meta.dev(), meta.ino()))
        };
        let agent_dir_path = dir.parent();
        let token = agent_dir_path.and_then(|parent| {
            let text = std::fs::read_to_string(parent.join("agent.json")).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            value
                .get("token")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
        Self {
            token,
            agent_dir: agent_dir_path.and_then(identity),
            events_dir: identity(dir),
        }
    }

    /// Whether `now` is a different record/log than this one.
    fn replaced(&self, now: &Self) -> bool {
        let changed = |before: &Option<String>, after: &Option<String>| matches!((before, after), (Some(a), Some(b)) if a != b);
        let dir_changed = |before: &Option<(u64, u64)>, after: &Option<(u64, u64)>| matches!((before, after), (Some(a), Some(b)) if a != b);
        changed(&self.token, &now.token)
            || dir_changed(&self.events_dir, &now.events_dir)
            || dir_changed(&self.agent_dir, &now.agent_dir)
    }
}

/// A headless target's event log being followed as its transcript (#1974).
struct HeadlessFollow {
    dir: PathBuf,
    follower: crate::headless::events::Follower,
    identity: HeadlessIdentity,
    /// Raw log events read from the log's bounded tail at open, converted at first poll.
    initial: Vec<crate::headless::events::Event>,
}

/// The coordinator's transcript, followed for its reply blocks.
pub(crate) struct CoordinatorTranscript {
    resolve: Resolve,
    discover: Option<Discover>,
    headless_dir: Option<HeadlessDir>,
    headless: Option<HeadlessFollow>,
    /// The event log directory of a headless target whose log has no segment yet, cached so
    /// its appearance is checked on every poll without reparsing its record.
    headless_pending_dir: Option<PathBuf>,
    /// Queued prompts' excerpts by queue id, until their `input.started` emits the prompt
    /// mark at the time the model actually started it.
    headless_prompts: VecDeque<(String, String)>,
    source: Option<SharedReplySource>,
    log: Log,
    follower: Option<Follower>,
    located_at: Option<Instant>,
    /// The last thing logged about what is read, so each change is logged once.
    reported: Option<String>,
    blocks: VecDeque<TranscriptBlock>,
    bytes: usize,
    waited: VecDeque<String>,
    events: VecDeque<Event>,
    collect_events: bool,
    trace: bool,
    ever_opened: bool,
    /// The agent followed, for a reader made by [`Self::for_agent`]; a repoint changes it.
    target: Option<SharedTarget>,
    /// Prompt and tool-call times, shared with the bridge state (#897).
    marks: Option<SharedTurnMarks>,
}

impl fmt::Debug for CoordinatorTranscript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoordinatorTranscript")
            .field("path", &self.follower.as_ref().map(Follower::path))
            .field("blocks", &self.blocks.len())
            .finish_non_exhaustive()
    }
}

impl CoordinatorTranscript {
    /// Follow the transcript of whatever `resolve` names, logging to `log`.
    pub(crate) fn new(resolve: Resolve, log: Log) -> Self {
        Self {
            resolve,
            discover: None,
            headless_dir: None,
            headless: None,
            headless_pending_dir: None,
            headless_prompts: VecDeque::new(),
            source: None,
            log,
            follower: None,
            located_at: None,
            reported: None,
            blocks: VecDeque::new(),
            bytes: 0,
            waited: VecDeque::new(),
            events: VecDeque::new(),
            collect_events: false,
            trace: false,
            ever_opened: false,
            target: None,
            marks: None,
        }
    }

    /// Share the prompt and tool-call times read from now on through `marks` (#897).
    #[must_use]
    pub(crate) fn with_turn_marks(mut self, marks: SharedTurnMarks) -> Self {
        self.marks = Some(marks);
        self
    }

    /// Tell the shared marks whether a transcript is followed now, forgetting what was read when
    /// `clear`.
    /// Whether any transcript is followed now: a harness file or a headless event log.
    fn following_any(&self) -> bool {
        self.follower.is_some() || self.headless.is_some()
    }

    fn share_following(&self, clear: bool) {
        if let Some(marks) = &self.marks {
            let mut marks = marks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            marks.following = self.following_any();
            if clear {
                marks.prompts.clear();
                marks.tools.clear();
            }
        }
    }

    /// Follow the transcript of agent `name` in `registry` (`None`: no target, so the reader
    /// rests), as its record describes it, or as [`discover`] finds it,
    /// confirmed by content against the reply IDs `expected` gives; sharing where the text comes
    /// from through `source`. A headless agent's transcript is its event log (#1974).
    /// [`TranscriptReplies::retarget`] changes the agent.
    pub(crate) fn for_agent(
        registry: PathBuf,
        name: Option<String>,
        log: Log,
        expected: Expected,
        source: SharedReplySource,
    ) -> Self {
        let target: SharedTarget = Arc::new(Mutex::new(name));
        let current = |target: &SharedTarget| {
            target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let resolve = {
            let (registry, target) = (registry.clone(), Arc::clone(&target));
            Box::new(move || record_hints(&registry, &current(&target)?)) as Resolve
        };
        let headless_dir = {
            let (registry, target) = (registry.clone(), Arc::clone(&target));
            Box::new(move || headless_events_dir(&registry, &current(&target)?)) as HeadlessDir
        };
        let mut transcript = Self::new(resolve, log);
        transcript.discover = Some({
            let target = Arc::clone(&target);
            Box::new(move || {
                discover(
                    &registry,
                    &current(&target)?,
                    Path::new("/proc"),
                    &crate::harness::claude::config_roots(),
                    &expected,
                )
            })
        });
        transcript.headless_dir = Some(headless_dir);
        transcript.source = Some(source);
        transcript.target = Some(target);
        transcript
    }

    /// Whether this reader rests because the bridge serves no target agent.
    fn resting(&self) -> bool {
        self.target.as_ref().is_some_and(|target| {
            target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        })
    }

    /// Discover the transcript with `discover` when `resolve` finds none.
    #[cfg(test)]
    fn with_discovery(mut self, discover: Discover, source: SharedReplySource) -> Self {
        self.discover = Some(discover);
        self.source = Some(source);
        self
    }

    fn share(&self, source: ReplySource) {
        if let Some(shared) = &self.source {
            *shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
        }
    }

    /// Follow a transcript for the trace (#714): collect every event, and
    /// start at the end of the file so old text is never replayed.
    pub(crate) fn for_agent_trace(registry: PathBuf, name: String, log: Log) -> Self {
        let mut source = Self::new(
            Box::new({
                let registry = registry.clone();
                let name = name.clone();
                move || record_hints(&registry, &name)
            }),
            log,
        );
        source.headless_dir = Some(Box::new(move || headless_events_dir(&registry, &name)));
        source.collect_events = true;
        source.trace = true;
        source
    }

    fn report(&mut self, line: String) {
        if self.reported.as_deref() != Some(line.as_str()) {
            (self.log)(line.clone());
            self.reported = Some(line);
        }
    }

    /// Follow `dir` as a headless target's transcript, or report that no log is followed yet.
    /// Whether the target is headless at all is re-resolved only on the harness relocate
    /// cadence (its record is not reparsed on every poll); once its directory is known, the
    /// log's appearance is checked on every poll from the cached path, so a wrapper that
    /// starts after the bridge switches the next prompt to marker replies with no bridge
    /// restart (#1974, owner 241).
    fn relocate_headless(&mut self, dir: PathBuf) {
        if self.headless.as_ref().is_some_and(|held| held.dir == dir) {
            return;
        }
        // A different log under a familiar path (a record replaced, #1221): nothing read of
        // the previous incarnation stays.
        if self.headless.is_some() {
            self.blocks.clear();
            self.bytes = 0;
            self.waited.clear();
            self.headless_prompts.clear();
        }
        self.follower = None;
        self.headless = None;
        match headless_start(&dir, self.trace, self.ever_opened) {
            Some(follow) => {
                self.ever_opened = true;
                self.headless_pending_dir = None;
                self.headless = Some(follow);
                self.share(ReplySource {
                    path: Some(dir.clone()),
                    evidence: HEADLESS_EVIDENCE.to_owned(),
                    headless_log: true,
                    ..ReplySource::default()
                });
                self.report(format!(
                    "agentctl: reply text comes from the headless agent's event log {}",
                    dir.display()
                ));
            }
            None => {
                self.headless_pending_dir = Some(dir);
                self.share(ReplySource {
                    evidence: HEADLESS_NO_LOG_EVIDENCE.to_owned(),
                    ..ReplySource::default()
                });
                self.report(
                    "agentctl: the bridge serves a headless agent whose event log is not \
                     followed yet; replies come only through `agentctl chat reply`"
                        .to_owned(),
                );
            }
        }
    }

    /// Whether the followed headless log's record/log was replaced since it was opened, or
    /// its generations regressed below the follower's cursor (a fresh log under the same
    /// name, #1221): the follower's byte offset must not be carried into the new log.
    fn headless_replaced(&self) -> bool {
        let Some(held) = &self.headless else {
            return false;
        };
        if held.identity.replaced(&HeadlessIdentity::of(&held.dir)) {
            return true;
        }
        let cursor = held.follower.cursor_generation();
        if cursor == 0 {
            return false;
        }
        let segments = crate::headless::events::segments(&held.dir).unwrap_or_default();
        !segments.is_empty()
            && segments
                .iter()
                .all(|segment| segment.key() < (cursor, u64::MAX))
            && segments.iter().map(|s| s.generation).max().unwrap_or(0) < cursor
    }

    /// Look for the transcript again when it is due, switching to another file if it moved.
    fn relocate(&mut self) {
        // Following a headless log: its identity is re-checked only on the relocate cadence;
        // the follower itself follows new segments on every poll.
        if self.headless.is_some() {
            if self
                .located_at
                .is_some_and(|at| at.elapsed() < RELOCATE_EVERY)
            {
                return;
            }
            self.located_at = Some(Instant::now());
            if !self.headless_replaced() {
                return;
            }
            let dir = self.headless.as_ref().expect("held log").dir.clone();
            self.headless = None;
            // Nothing read of the replaced incarnation stays.
            self.blocks.clear();
            self.bytes = 0;
            self.waited.clear();
            self.headless_prompts.clear();
            self.relocate_headless(dir);
            return;
        }
        // A headless target whose log has not appeared: look for its first segment on every
        // poll, from the cached directory (no record reparse).
        if let Some(dir) = self.headless_pending_dir.clone() {
            self.relocate_headless(dir);
            return;
        }
        // Looking reads the process table and the record, so it is done at most once per
        // period, found or not.
        if self
            .located_at
            .is_some_and(|at| at.elapsed() < RELOCATE_EVERY)
        {
            return;
        }
        self.located_at = Some(Instant::now());
        if let Some(dir) = self.headless_dir.as_ref().and_then(|resolve| resolve()) {
            self.relocate_headless(dir);
            return;
        }
        if self.resting() {
            self.follower = None;
            self.headless = None;
            self.share(ReplySource {
                evidence: RESTING_EVIDENCE.to_owned(),
                ..ReplySource::default()
            });
            self.report(
                "agentctl: the bridge serves no target agent; no transcript is read".to_owned(),
            );
            return;
        }
        self.headless = None;
        let resolved = (self.resolve)();
        // A harness agentctl reads no transcript of, such as one driven by the generic screen
        // rules (`--harness unknown`): its reply text can only come from its screen.
        let unread_harness = resolved
            .as_ref()
            .map(|(harness, _)| harness.clone())
            .filter(|harness| agent_harness::transcript(harness).is_none());
        let found = resolved
            .and_then(|(harness, hints)| {
                let reader = agent_harness::transcript(&harness)?;
                let located = reader.locate(&hints)?;
                // Without a recorded conversation, Claude Code's file is found only through the
                // live process that names it: a conversation the record lacks.
                let bind = (hints.session.is_none() && harness == "claude")
                    .then(|| session_of(&located.path))
                    .flatten();
                Some((reader, located, false, bind))
            })
            .or_else(|| {
                let found = (self.discover.as_ref()?)()?;
                let reader = agent_harness::transcript(&found.harness)?;
                Some((reader, found.located, true, found.bind))
            });
        let Some((reader, found, discovered, bind)) = found else {
            self.follower = None;
            let line = if self.trace {
                "agentctl: no transcript of the coordinator was found; the trace is idle"
            } else {
                "agentctl: no transcript of the coordinator was found; replies keep the \
                        text read from its screen"
            };
            // An agentcloud session's replies come from the session's own history, which the
            // bridge reads in place of a screen (`ManagedAgents::read_capture`).
            let cloud =
                crate::harness::HarnessAdapter::name(&crate::harness::agentcloud::AgentCloud);
            if unread_harness.as_deref() == Some(cloud) {
                self.share(ReplySource {
                    evidence: CLOUD_EVIDENCE.to_owned(),
                    session_history: true,
                    ..ReplySource::default()
                });
                self.report(
                    "agentctl: the coordinator is an agentcloud session; replies are read from \
                     its session history"
                        .to_owned(),
                );
                return;
            }
            let evidence = match unread_harness {
                Some(harness) => format!(
                    "the target's harness ({harness}) writes no transcript agentctl reads: its \
                     replies come from CHAT_REPLY blocks on its screen, or through `agentctl chat \
                     reply`"
                ),
                None => "no transcript of the coordinator was found: its record names no \
                         conversation or process that finds one, and none was discovered"
                    .to_owned(),
            };
            self.share(ReplySource {
                evidence,
                ..ReplySource::default()
            });
            self.report(line.to_owned());
            return;
        };
        self.share(ReplySource {
            path: Some(found.path.clone()),
            evidence: found.evidence.clone(),
            discovered,
            unbound_session: bind,
            session_history: false,
            headless_log: false,
        });
        if self
            .follower
            .as_ref()
            .is_some_and(|follower| follower.path() == found.path)
        {
            return;
        }
        // Only the trace's first open starts at EOF; later opens (a new
        // session file, a reopen after an error) reread the tail, and the
        // trace's high-water mark filters what it has already posted.
        let tail = if self.trace && !self.ever_opened {
            0
        } else {
            FIRST_READ_BYTES
        };
        self.ever_opened = true;
        self.follower = Some(Follower::open(&found.path, reader.decoder(), tail));
        self.report(format!(
            "agentctl: reply text comes from the coordinator's {} transcript {} ({}{})",
            reader.name(),
            found.path.display(),
            found.evidence,
            if discovered {
                "; discovered: the coordinator's record names no conversation that finds it"
            } else {
                ""
            }
        ));
    }

    /// Headless log events as harness-neutral events, in order. An `input.queued` only
    /// remembers its prompt's excerpt; the prompt mark is emitted at its `input.started`,
    /// when the model actually started it, with that excerpt as its text.
    fn convert_headless(&mut self, raw: &[crate::headless::events::Event]) -> Vec<Event> {
        let mut events = Vec::new();
        for event in raw {
            match event.kind.as_str() {
                "input.queued" => {
                    let queue = event.data["queue_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| event.causation.clone());
                    let excerpt = event.data["excerpt"].as_str().unwrap_or_default();
                    if let (Some(queue), false) = (queue, excerpt.is_empty()) {
                        self.headless_prompts.push_back((queue, excerpt.to_owned()));
                        while self.headless_prompts.len() > 64 {
                            self.headless_prompts.pop_front();
                        }
                    }
                }
                "input.started" => {
                    let queue = event.data["queue_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| event.causation.clone());
                    let text = queue
                        .and_then(|queue| {
                            self.headless_prompts
                                .iter()
                                .position(|(held, _)| *held == queue)
                                .map(|at| self.headless_prompts.remove(at).expect("held").1)
                        })
                        .unwrap_or_default();
                    events.push(Event::Prompt {
                        at: Some(event.ts as f64 / 1000.0),
                        text,
                        via: agent_harness::Via::Typed,
                    });
                }
                _ => {
                    if let Some(event) = headless_event(event) {
                        events.push(event);
                    }
                }
            }
        }
        events
    }

    /// Read what the transcript gained, keeping its complete reply blocks.
    fn poll(&mut self) {
        self.relocate();
        self.share_following(false);
        let events = if self.headless.is_some() {
            let (initial, polled) = {
                let held = self.headless.as_mut().expect("held log");
                let initial = std::mem::take(&mut held.initial);
                match held.follower.poll() {
                    Ok(raw) => (initial, raw),
                    Err(error) => {
                        let dir = held.dir.clone();
                        self.headless = None;
                        self.headless_pending_dir = Some(dir.clone());
                        self.share_following(false);
                        self.share(ReplySource {
                            evidence: HEADLESS_NO_LOG_EVIDENCE.to_owned(),
                            ..ReplySource::default()
                        });
                        self.report(format!(
                            "agentctl: the headless agent's event log {} cannot be read \
                             ({error}); replies come only through `agentctl chat reply`",
                            dir.display()
                        ));
                        return;
                    }
                }
            };
            let mut events = self.convert_headless(&initial);
            events.extend(self.convert_headless(&polled));
            events
        } else if let Some(follower) = self.follower.as_mut() {
            match follower.poll() {
                Ok(events) => events,
                Err(error) => {
                    let path = follower.path().display().to_string();
                    self.follower = None;
                    self.share_following(false);
                    self.report(format!(
                        "agentctl: the coordinator's transcript {path} cannot be read ({error}); {}",
                        if self.trace {
                            "the trace is idle"
                        } else {
                            "replies keep the text read from its screen"
                        }
                    ));
                    return;
                }
            }
        } else {
            return;
        };
        if let Some(marks) = &self.marks {
            let mut marks = marks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for event in &events {
                marks.note(event);
            }
        }
        for event in events {
            if self.collect_events {
                self.events.push_back(event.clone());
                while self.events.len() > 1024 {
                    self.events.pop_front();
                }
            }
            let Event::Text { at, text } = event else {
                continue;
            };
            for block in replies::complete_blocks(&text) {
                let block = TranscriptBlock {
                    identifier: block.identifier,
                    body: block.body,
                    at_millis: at.filter(|at| *at >= 0.0).map(|at| (at * 1000.0) as u64),
                };
                // A tail read again, after a failure or a switch back, repeats blocks.
                if self.blocks.contains(&block) {
                    continue;
                }
                self.bytes += block.body.len();
                self.blocks.push_back(block);
            }
        }
        while self.blocks.len() > MAX_BLOCKS || self.bytes > MAX_BLOCK_BYTES {
            let Some(oldest) = self.blocks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(oldest.body.len());
        }
    }

    /// Drain the transcript events read since the last call (#714).
    pub(crate) fn events(&mut self) -> Vec<Event> {
        self.poll();
        self.events.drain(..).collect()
    }

    fn holds(&self, identifier: &str) -> bool {
        self.blocks
            .iter()
            .any(|block| block.identifier == identifier)
    }
}

impl TranscriptReplies for CoordinatorTranscript {
    fn blocks(&mut self) -> Vec<TranscriptBlock> {
        self.poll();
        self.blocks.iter().cloned().collect()
    }

    fn settle(&mut self, identifiers: &[String]) {
        let pending: Vec<&String> = identifiers
            .iter()
            .filter(|identifier| !self.waited.contains(identifier))
            .collect();
        if pending.is_empty() {
            return;
        }
        let deadline = Instant::now() + SETTLE_FOR;
        loop {
            self.poll();
            if !self.following_any()
                || pending.iter().all(|identifier| self.holds(identifier))
                || Instant::now() >= deadline
            {
                break;
            }
            std::thread::sleep(SETTLE_STEP);
        }
        for identifier in pending {
            self.waited.push_back(identifier.clone());
        }
        while self.waited.len() > MAX_WAITED {
            self.waited.pop_front();
        }
    }

    fn following(&self) -> bool {
        self.following_any()
    }

    fn retarget(&mut self, agent: Option<&str>) {
        let Some(target) = &self.target else {
            return;
        };
        {
            let mut current = target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if current.as_deref() == agent {
                return;
            }
            *current = agent.map(str::to_owned);
        }
        // Nothing read of the previous agent stays: its blocks are not this agent's replies, and
        // a file still followed would hold this agent's screen replies back as unpaired.
        self.follower = None;
        self.headless = None;
        self.headless_pending_dir = None;
        self.headless_prompts.clear();
        self.blocks.clear();
        self.bytes = 0;
        self.waited.clear();
        self.reported = None;
        self.located_at = None;
        self.share_following(true);
        (self.log)(format!(
            "agentctl: the bridge now serves {}; its reply text is looked for again",
            agent.unwrap_or("no target agent")
        ));
        self.relocate();
    }
}

/// The event log directory of agent `name` in `registry` when its record says it is a
/// headless agent, else `None` (#1974).
fn headless_events_dir(registry: &Path, name: &str) -> Option<PathBuf> {
    let directory = agent_queue::registry::directory(registry, name).ok()?;
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("agent.json")).ok()?).ok()?;
    let headless = record["adapter"].as_str() == Some(crate::headless::ADAPTER)
        || record["mode"].as_str() == Some(crate::headless::MODE);
    headless.then(|| crate::headless::events::directory(&directory))
}

/// Open a follower of the headless log in `dir`, bounded so a first open never reads a whole
/// segment history on the caller's thread: for the trace's first open it starts at the end
/// (old text is never replayed); otherwise the last [`FIRST_READ_BYTES`] are read once with
/// `events::read_recent` as the initial events and the follower starts at the end, so replies
/// written while the bridge was down are found. Events in both are deduplicated downstream
/// by block identity. `None` when the log has no segment yet or cannot be listed.
fn headless_start(dir: &Path, trace: bool, ever_opened: bool) -> Option<HeadlessFollow> {
    let segments = crate::headless::events::segments(dir).ok()?;
    if segments.is_empty() {
        return None;
    }
    let identity = HeadlessIdentity::of(dir);
    let (follower, initial) = if trace && !ever_opened {
        (
            crate::headless::events::Follower::at_end(dir).ok()?,
            Vec::new(),
        )
    } else {
        let follower = crate::headless::events::Follower::at_end(dir).ok()?;
        let initial =
            crate::headless::events::read_recent(dir, FIRST_READ_BYTES).unwrap_or_default();
        (follower, initial)
    };
    Some(HeadlessFollow {
        dir: dir.to_path_buf(),
        follower,
        identity,
        initial,
    })
}

/// One headless log event as the harness-neutral event the reply and trace paths read:
/// assistant text, tool call and turn end carry over (#1974); prompts are converted by the
/// reader from their `input.started` (see `convert_headless`). Assistant text is
/// verbatim, so reply blocks sliced from it need no screen rendering.
fn headless_event(event: &crate::headless::events::Event) -> Option<Event> {
    let at = Some(event.ts as f64 / 1000.0);
    let text = |key: &str| event.data[key].as_str().unwrap_or_default().to_owned();
    match event.kind.as_str() {
        "assistant.text" => Some(Event::Text {
            at,
            text: text("text"),
        }),
        "tool.started" => Some(Event::ToolCall {
            at,
            id: text("tool_use_id"),
            name: text("tool"),
        }),
        "turn.completed" => Some(Event::TurnEnd { at }),
        _ => None,
    }
}

/// The harness and transcript hints of agent `name` in `registry`, from its record: the pinned
/// harness process, its working directory, and the conversation id it records, if any.
fn record_hints(registry: &Path, name: &str) -> Option<(String, Hints)> {
    let path = agent_queue::registry::directory(registry, name)
        .ok()?
        .join("agent.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut harness = record["harness"].as_str()?.to_owned();
    // A pane driven by the generic rules that shows an agentcloud session through agentterm:
    // its replies are read from that session's history, as a cloud target's are (#852 D).
    if crate::unknown_harness::is_unknown(&harness)
        && record["pane_id"]
            .as_str()
            .and_then(crate::subagents::pane_views_session)
            .is_some()
    {
        harness = crate::harness::HarnessAdapter::name(&crate::harness::agentcloud::AgentCloud)
            .to_owned();
    }
    let session = record_hints_session(&record);
    let hints = Hints {
        pid: record
            .pointer("/harness_identity/pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        cwd: record["cwd"].as_str().map(PathBuf::from),
        session,
        ..Hints::default()
    };
    Some((harness, hints))
}

/// The harness of agent `name` in `registry` and the conversation its record names, if any, for
/// `chat health`; `None` when the record cannot be read.
pub(crate) fn recorded_conversation(
    registry: &Path,
    name: &str,
) -> Option<(String, Option<String>)> {
    let path = agent_queue::registry::directory(registry, name)
        .ok()?
        .join("agent.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((
        record["harness"].as_str()?.to_owned(),
        record_hints_session(&record),
    ))
}

/// The conversation id of a Claude Code transcript: its file's stem.
fn session_of(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty() && !stem.contains(['/', '\0']))
        .map(str::to_owned)
}

/// The transcript of agent `name` in `registry` when its record finds none (#783
/// chat-reply-capture-miss), looked for in two ways:
///
/// 1. through the processes in `proc_root` whose environment names the agent's Herdr pane
///    (`HERDR_PANE_ID`), lowest process id first, located as the record's own process would be;
/// 2. for Claude Code, as the newest of the [`DISCOVER_FILES`] most recently written transcripts
///    of conversations begun in the agent's working directory under any of `claude_configs`
///    (listed by the lister #741's revive discovery uses) whose last
///    [`DISCOVER_TAIL_BYTES`] mention `CHAT_REPLY_<ID>>` for an ID `expected` gives: a request
///    the bridge delivered to the agent and that has no reply yet.
///
/// Only the first, for a Claude Code record that names no conversation, offers the conversation
/// for binding.
fn discover(
    registry: &Path,
    name: &str,
    proc_root: &Path,
    claude_configs: &[PathBuf],
    expected: &dyn Fn() -> Vec<String>,
) -> Option<Discovered> {
    let path = agent_queue::registry::directory(registry, name)
        .ok()?
        .join("agent.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let harness = record["harness"].as_str()?.to_owned();
    let reader = agent_harness::transcript(&harness)?;
    let cwd = record["cwd"].as_str().map(PathBuf::from);
    let names_session = record_hints_session(&record).is_some();
    if let Some(pane) = record["pane_id"].as_str().filter(|pane| !pane.is_empty()) {
        for pid in pane_processes(proc_root, pane) {
            let hints = Hints {
                pid: Some(pid),
                cwd: cwd.clone(),
                proc_root: proc_root.to_path_buf(),
                ..Hints::default()
            };
            if let Some(mut located) = reader.locate(&hints) {
                located.evidence = format!("{}, running in its pane {pane}", located.evidence);
                let bind = (harness == "claude" && !names_session)
                    .then(|| session_of(&located.path))
                    .flatten();
                return Some(Discovered {
                    harness,
                    located,
                    bind,
                });
            }
        }
    }
    if harness != "claude" {
        return None;
    }
    let cwd = cwd?;
    let expected = expected();
    if expected.is_empty() {
        return None;
    }
    let transcripts =
        crate::harness::claude::project_transcripts(claude_configs, &cwd, &|stem: &str| {
            !stem.is_empty() && !stem.contains(['/', '\0'])
        });
    for transcript in transcripts.into_iter().take(DISCOVER_FILES) {
        let Some(tail) = tail_text(&transcript.path, DISCOVER_TAIL_BYTES) else {
            continue;
        };
        if let Some(identifier) = expected
            .iter()
            .find(|identifier| tail.contains(&format!("CHAT_REPLY_{identifier}>")))
        {
            let evidence = format!(
                "the newest transcript of a conversation begun in {} that mentions reply ID \
                 {identifier} of a request delivered to the agent",
                cwd.display()
            );
            return Some(Discovered {
                harness,
                located: Located {
                    path: transcript.path,
                    evidence,
                },
                bind: None,
            });
        }
    }
    None
}

/// The conversation id a record names: recorded as a string, or as an object holding it under
/// `value`.
fn record_hints_session(record: &serde_json::Value) -> Option<String> {
    ["native_session", "session_value", "resume"]
        .iter()
        .find_map(|key| {
            let value = &record[*key];
            value
                .as_str()
                .or_else(|| value["value"].as_str())
                .filter(|value| !value.is_empty())
        })
        .map(str::to_owned)
}

/// The processes in `proc_root` whose environment names the Herdr pane `pane`, lowest first.
fn pane_processes(proc_root: &Path, pane: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut found: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            crate::inbox::environment_value(proc_root, &pid.to_string(), "HERDR_PANE_ID")
                .is_some_and(|value| value == pane)
        })
        .collect();
    found.sort_unstable();
    found
}

/// The last `bytes` of the file at `path`, as text.
fn tail_text(path: &Path, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let from = length.saturating_sub(bytes);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut text = Vec::new();
    file.take(length - from).read_to_end(&mut text).ok()?;
    Some(String::from_utf8_lossy(&text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentctl-chat-transcript-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One Claude Code assistant entry holding `text`.
    fn entry(text: &str, at: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": at,
            "message": {"content": [{"type": "text", "text": text}]},
        })
        .to_string()
            + "\n"
    }

    fn source_for(path: PathBuf, logs: Arc<Mutex<Vec<String>>>) -> CoordinatorTranscript {
        CoordinatorTranscript::new(
            Box::new(move || {
                Some((
                    "claude".to_owned(),
                    Hints {
                        session: Some("s-1".to_owned()),
                        home: Some(path.clone()),
                        ..Hints::default()
                    },
                ))
            }),
            Box::new(move |line| logs.lock().unwrap().push(line)),
        )
    }

    /// A reader that follows whichever agent its shared target names: agent `NAME`'s Claude
    /// Code transcript is `homes/NAME/.claude/projects/-work/s-1.jsonl`, and agent `odd` runs a
    /// harness agentctl reads no transcript of.
    fn targeted(
        homes: PathBuf,
        name: Option<&str>,
        logs: Arc<Mutex<Vec<String>>>,
    ) -> (CoordinatorTranscript, SharedReplySource) {
        let target: SharedTarget = Arc::new(Mutex::new(name.map(str::to_owned)));
        let resolve = {
            let target = Arc::clone(&target);
            Box::new(move || {
                let name = target.lock().unwrap().clone()?;
                let harness = if name == "odd" { "unknown" } else { "claude" };
                Some((
                    harness.to_owned(),
                    Hints {
                        session: Some("s-1".to_owned()),
                        home: Some(homes.join(&name)),
                        ..Hints::default()
                    },
                ))
            }) as Resolve
        };
        let source = SharedReplySource::default();
        let mut transcript = CoordinatorTranscript::new(
            resolve,
            Box::new(move |line| logs.lock().unwrap().push(line)),
        );
        transcript.source = Some(Arc::clone(&source));
        transcript.target = Some(target);
        (transcript, source)
    }

    #[test]
    fn a_repoint_follows_the_new_agent_and_drops_what_was_read_of_the_previous() {
        // #852 unknown-tui-bridge: the reader follows the agent the bridge serves now; nothing
        // read of the previous agent stays, and with no target agent the reader rests.
        let root = scratch("retarget");
        for (name, identifier) in [("coordb", "007"), ("liaison", "008")] {
            let project = root.join(format!("{name}/.claude/projects/-work"));
            fs::create_dir_all(&project).unwrap();
            fs::write(
                project.join("s-1.jsonl"),
                entry(
                    &format!("<CHAT_REPLY_{identifier}>\nfrom {name}\n</CHAT_REPLY_{identifier}>"),
                    "2026-11-04T12:00:00Z",
                ),
            )
            .unwrap();
        }
        let logs = Arc::new(Mutex::new(Vec::new()));
        let (mut reader, source) = targeted(root.clone(), Some("coordb"), Arc::clone(&logs));
        let identifiers = |reader: &mut CoordinatorTranscript| {
            reader
                .blocks()
                .into_iter()
                .map(|block| block.identifier)
                .collect::<Vec<_>>()
        };
        assert_eq!(identifiers(&mut reader), ["007"]);
        let path = |source: &SharedReplySource| source.lock().unwrap().clone().unwrap().path;
        assert_eq!(
            path(&source),
            Some(root.join("coordb/.claude/projects/-work/s-1.jsonl"))
        );

        reader.retarget(Some("liaison"));
        assert!(reader.following());
        assert_eq!(identifiers(&mut reader), ["008"]);
        assert_eq!(
            path(&source),
            Some(root.join("liaison/.claude/projects/-work/s-1.jsonl"))
        );
        assert!(logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("now serves liaison")));
        // The same target again changes nothing.
        let logged = logs.lock().unwrap().len();
        reader.retarget(Some("liaison"));
        assert_eq!(logs.lock().unwrap().len(), logged);

        reader.retarget(None);
        assert!(!reader.following());
        assert!(identifiers(&mut reader).is_empty());
        let rested = source.lock().unwrap().clone().unwrap();
        assert_eq!(rested.path, None);
        assert_eq!(rested.evidence, RESTING_EVIDENCE);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_agentcloud_targets_replies_come_from_its_session_history() {
        // An agentcloud session writes no local transcript: the bridge reads its replies from the
        // session's own history in place of a screen (`ManagedAgents::read_capture`), so the
        // source says so, and nothing is followed that would hold those replies back.
        let root = scratch("cloud-target");
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "agentcloud", "cwd": "/work", "pane_id": "wJ:p1"}),
        );
        let logs = Arc::new(Mutex::new(Vec::new()));
        let source = SharedReplySource::default();
        let mut reader = CoordinatorTranscript::for_agent(
            registry,
            Some("coord".to_owned()),
            Box::new({
                let logs = Arc::clone(&logs);
                move |line| logs.lock().unwrap().push(line)
            }),
            Box::new(Vec::new),
            Arc::clone(&source),
        );
        assert!(reader.blocks().is_empty());
        assert!(!reader.following());
        let found = source.lock().unwrap().clone().unwrap();
        assert!(found.session_history);
        assert_eq!(found.path, None);
        assert_eq!(found.evidence, CLOUD_EVIDENCE);
        assert!(logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("replies are read from its session history")));
        let saved = serde_json::to_value(&found).unwrap();
        assert_eq!(saved["session_history"], true);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_target_whose_harness_writes_no_transcript_replies_through_its_screen() {
        // #852 unknown-tui-bridge: an agent driven by the generic screen rules has no transcript
        // agentctl reads, so its reply text comes from its screen, and the source says so.
        let root = scratch("retarget-unknown");
        let project = root.join("coordb/.claude/projects/-work");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("s-1.jsonl"),
            entry(
                "<CHAT_REPLY_007>\nold\n</CHAT_REPLY_007>",
                "2026-11-04T12:00:00Z",
            ),
        )
        .unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let (mut reader, source) = targeted(root.clone(), Some("coordb"), logs);
        assert_eq!(reader.blocks().len(), 1);
        reader.retarget(Some("odd"));
        assert!(!reader.following());
        assert!(reader.blocks().is_empty());
        let found = source.lock().unwrap().clone().unwrap();
        assert_eq!(found.path, None);
        assert!(
            found
                .evidence
                .contains("(unknown) writes no transcript agentctl reads"),
            "{}",
            found.evidence
        );
        assert!(
            found.evidence.contains("agentctl chat reply"),
            "{}",
            found.evidence
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_headless_targets_event_log_is_followed_as_its_transcript() {
        // #1974: a headless agent has no harness transcript the bridge reads; its wrapper's
        // event log is the transcript. Before any event, no log is followed (command-only
        // prompts); once `assistant.text` events appear, their complete reply blocks are kept
        // verbatim, and the same events feed the trace as prompt/text/tool/turn-end.
        let root = scratch("headless-log");
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "adapter": "harness-wrapper", "mode": "headless", "cwd": "/work"}),
        );
        let logs = Arc::new(Mutex::new(Vec::new()));
        let source = SharedReplySource::default();
        let mut reader = CoordinatorTranscript::for_agent(
            registry.clone(),
            Some("coord".to_owned()),
            Box::new({
                let logs = Arc::clone(&logs);
                move |line| logs.lock().unwrap().push(line)
            }),
            Box::new(Vec::new),
            Arc::clone(&source),
        );
        assert!(reader.blocks().is_empty());
        assert!(!reader.following());
        let found = source.lock().unwrap().clone().unwrap();
        assert_eq!(found.path, None);
        assert_eq!(found.evidence, HEADLESS_NO_LOG_EVIDENCE);
        assert!(!found.headless_log);

        let events_dir = crate::headless::events::directory(&registry.join("agents/coord"));
        let mut writer = crate::headless::events::EventWriter::open(&events_dir, "coord").unwrap();
        writer
            .append(
                "input.queued",
                Some("q1"),
                serde_json::json!({"excerpt": "[from Owner via slack #test · reply: <CHAT_REPLY_001>]"}),
            )
            .unwrap();
        writer
            .append(
                "input.started",
                Some("q1"),
                serde_json::json!({"queue_id": "q1", "harness_input_id": "u1", "mid_turn": false}),
            )
            .unwrap();
        writer
            .append(
                "assistant.text",
                Some("q1"),
                serde_json::json!({"text": "Working note.\n<CHAT_REPLY_001>\n**Done.**\n</CHAT_REPLY_001>\nTrail."}),
            )
            .unwrap();
        writer
            .append(
                "tool.started",
                Some("q1"),
                serde_json::json!({"tool": "Bash", "tool_use_id": "t1"}),
            )
            .unwrap();
        writer
            .append("turn.completed", Some("q1"), serde_json::json!({}))
            .unwrap();

        reader.collect_events = true;
        let events = reader.events();
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(matches!(&events[0], Event::Prompt { .. }));
        assert!(matches!(&events[1], Event::Text { .. }));
        assert!(matches!(&events[2], Event::ToolCall { .. }));
        assert!(matches!(&events[3], Event::TurnEnd { .. }));
        // The trace harvest (#1608 via #714) gets only the outside-marker text.
        let mut collector = crate::chat_trace::Collector::new(Duration::from_secs(45));
        let mut posts = Vec::new();
        for event in &events {
            posts.extend(collector.feed(event));
        }
        let traced: Vec<&str> = posts.iter().map(|post| post.text.as_str()).collect();
        assert!(
            traced.iter().any(|text| text.contains("Working note.")),
            "{traced:?}"
        );
        assert!(
            !traced.iter().any(|text| text.contains("**Done.**")),
            "{traced:?}"
        );
        assert!(reader.following());
        let blocks = reader.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].identifier, "001");
        assert_eq!(blocks[0].body, "**Done.**");
        assert!(blocks[0].at_millis.is_some_and(|at| at > 0));
        let found = source.lock().unwrap().clone().unwrap();
        assert_eq!(found.path, Some(events_dir.clone()));
        assert!(found.headless_log);
        assert_eq!(found.evidence, HEADLESS_EVIDENCE);

        writer
            .append(
                "assistant.text",
                None,
                serde_json::json!({"text": "<CHAT_REPLY_002>\nMore\n</CHAT_REPLY_002>"}),
            )
            .unwrap();
        assert_eq!(reader.blocks().len(), 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_replaced_headless_log_under_the_same_name_is_read_from_its_start() {
        // #1974 review (as #1221): a record replaced under the same name gets a new token
        // and a fresh log at generation 1; the old follower's byte offset must not silently
        // skip the new log's early assistant text.
        let root = scratch("headless-replaced");
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "adapter": "harness-wrapper", "mode": "headless", "cwd": "/work", "token": "t1"}),
        );
        let source = SharedReplySource::default();
        let mut reader = CoordinatorTranscript::for_agent(
            registry.clone(),
            Some("coord".to_owned()),
            Box::new(|_| {}),
            Box::new(Vec::new),
            Arc::clone(&source),
        );
        let events_dir = crate::headless::events::directory(&registry.join("agents/coord"));
        let mut writer = crate::headless::events::EventWriter::open(&events_dir, "coord").unwrap();
        writer
            .append(
                "assistant.text",
                None,
                serde_json::json!({"text": "<CHAT_REPLY_001>\nold\n</CHAT_REPLY_001>"}),
            )
            .unwrap();
        assert_eq!(reader.blocks().len(), 1);
        drop(writer);

        // Replace the record (new token) and its log under the same name and path.
        fs::write(
            registry.join("agents/coord/agent.json"),
            serde_json::json!({"harness": "claude", "adapter": "harness-wrapper", "mode": "headless", "cwd": "/work", "token": "t2"})
                .to_string(),
        )
        .unwrap();
        fs::remove_dir_all(&events_dir).unwrap();
        let mut writer = crate::headless::events::EventWriter::open(&events_dir, "coord").unwrap();
        writer
            .append(
                "assistant.text",
                None,
                serde_json::json!({"text": "<CHAT_REPLY_009>\nnew\n</CHAT_REPLY_009>"}),
            )
            .unwrap();
        // Force the relocate-cadence identity re-check.
        reader.located_at = Some(Instant::now() - RELOCATE_EVERY - Duration::from_secs(1));
        let blocks = reader.blocks();
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.identifier.as_str())
                .collect::<Vec<_>>(),
            ["009"],
            "{blocks:?}"
        );
        assert!(reader.following());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_headless_trace_reads_only_new_event_log_text() {
        // #1974 trace side: the trace reader for a headless agent follows the same event
        // log, starting at its end so old text is never replayed to the trace thread.
        let root = scratch("headless-trace");
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "adapter": "harness-wrapper", "mode": "headless", "cwd": "/work"}),
        );
        let events_dir = crate::headless::events::directory(&registry.join("agents/coord"));
        let mut writer = crate::headless::events::EventWriter::open(&events_dir, "coord").unwrap();
        writer
            .append(
                "assistant.text",
                None,
                serde_json::json!({"text": "old text"}),
            )
            .unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let mut reader = CoordinatorTranscript::for_agent_trace(
            registry.clone(),
            "coord".to_owned(),
            Box::new({
                let logs = Arc::clone(&logs);
                move |line| logs.lock().unwrap().push(line)
            }),
        );
        assert!(reader.events().is_empty(), "old text replayed");
        writer
            .append(
                "assistant.text",
                None,
                serde_json::json!({"text": "new text"}),
            )
            .unwrap();
        let events = reader.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(matches!(&events[0], Event::Text { text, .. } if text == "new text"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn complete_blocks_are_kept_with_their_times_and_each_change_is_logged_once() {
        let home = scratch("blocks");
        let project = home.join(".claude/projects/-work");
        fs::create_dir_all(&project).unwrap();
        let path = project.join("s-1.jsonl");
        fs::write(
            &path,
            entry(
                "Note.\n<CHAT_REPLY_007>\n**Done.**\n</CHAT_REPLY_007>",
                "2026-10-30T12:00:00Z",
            ),
        )
        .unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let mut source = source_for(home.clone(), Arc::clone(&logs));
        let blocks = source.blocks();
        assert_eq!(
            blocks,
            [TranscriptBlock {
                identifier: "007".into(),
                body: "**Done.**".into(),
                at_millis: Some(1_793_361_600_000),
            }]
        );
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(
            entry(
                "<CHAT_REPLY_008>\nMore\n</CHAT_REPLY_008>",
                "2026-10-30T12:00:01Z",
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(source.blocks().len(), 2);
        assert_eq!(logs.lock().unwrap().len(), 1, "{:?}", logs.lock().unwrap());
        assert!(logs.lock().unwrap()[0].contains("s-1.jsonl"));
        // A transcript that disappears leaves the blocks read and is reported once.
        fs::remove_file(&path).unwrap();
        assert_eq!(source.blocks().len(), 2);
        assert_eq!(source.blocks().len(), 2);
        let logged = logs.lock().unwrap().clone();
        assert_eq!(logged.len(), 2, "{logged:?}");
        assert!(logged[1].contains("cannot be read"), "{logged:?}");
        fs::remove_dir_all(home).ok();
    }

    #[test]
    fn events_are_collected_only_for_the_trace() {
        let home = scratch("events");
        let project = home.join(".claude/projects/-work");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("s-1.jsonl"),
            entry("hello", "2026-10-30T12:00:00Z"),
        )
        .unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        // The reply path collects no events (flag off is unchanged).
        let mut reply_source = source_for(home.clone(), Arc::clone(&logs));
        assert!(reply_source.events().is_empty());
        // The trace source collects them, starting at EOF on first read is
        // covered by the follower tail; here the file predates the open.
        let mut trace_source = source_for(home.clone(), Arc::clone(&logs));
        trace_source.collect_events = true;
        trace_source.trace = true;
        // Tail 0: nothing that predates the open is replayed.
        assert!(trace_source.events().is_empty());
        let path = project.join("s-1.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(entry("later", "2026-10-30T12:00:01Z").as_bytes())
            .unwrap();
        assert!(!trace_source.events().is_empty());
        fs::remove_dir_all(home).ok();
    }

    #[test]
    fn settling_waits_once_per_id_and_not_without_a_transcript() {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&logs);
        let mut source = CoordinatorTranscript::new(
            Box::new(|| None),
            Box::new(move |line| log.lock().unwrap().push(line)),
        );
        let started = Instant::now();
        source.settle(&["001".to_owned()]);
        assert!(started.elapsed() < SETTLE_FOR, "no transcript: no wait");
        assert!(source.blocks().is_empty());
        assert_eq!(logs.lock().unwrap().len(), 1);

        let home = scratch("settle");
        let project = home.join(".claude/projects/-work");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("s-1.jsonl"), "").unwrap();
        let mut source = source_for(home.clone(), Arc::new(Mutex::new(Vec::new())));
        let started = Instant::now();
        source.settle(&["002".to_owned()]);
        assert!(started.elapsed() >= SETTLE_FOR, "waits for a missing block");
        let started = Instant::now();
        source.settle(&["002".to_owned()]);
        assert!(started.elapsed() < SETTLE_FOR, "but only once");
        fs::remove_dir_all(home).ok();
    }

    #[test]
    fn a_record_gives_the_harness_process_directory_and_conversation() {
        let registry = scratch("record");
        let directory = registry.join("agents/coord");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("agent.json"),
            r#"{"name":"coord","harness":"muse","cwd":"/work","native_session":{"agent":"muse","value":"s-9"},"session_value":null,"harness_identity":{"pid":4242}}"#,
        )
        .unwrap();
        let (harness, hints) = record_hints(&registry, "coord").unwrap();
        assert_eq!(harness, "muse");
        assert_eq!(hints.pid, Some(4242));
        assert_eq!(hints.cwd.as_deref(), Some(Path::new("/work")));
        assert_eq!(hints.session.as_deref(), Some("s-9"));
        assert_eq!(record_hints(&registry, "absent"), None);
        fs::remove_dir_all(registry).ok();
    }

    /// A registry holding agent `coord` with the record `record`.
    fn registry_with(root: &Path, record: &serde_json::Value) -> PathBuf {
        let registry = root.join("registry");
        let directory = registry.join("agents/coord");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("agent.json"), record.to_string()).unwrap();
        registry
    }

    #[test]
    fn a_record_naming_no_conversation_is_discovered_through_the_process_in_its_pane() {
        let root = scratch("discover-pane");
        let home = root.join("home");
        let proc_root = root.join("proc");
        // Process 4242 runs in pane wX:p1 and names its conversation; 4100 runs in another pane.
        for (pid, pane) in [(4242, "wX:p1"), (4100, "wX:p9")] {
            fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
            fs::write(
                proc_root.join(format!("{pid}/environ")),
                format!("HOME={}\0HERDR_PANE_ID={pane}\0", home.display()),
            )
            .unwrap();
            fs::create_dir_all(home.join(".claude/sessions")).unwrap();
            fs::write(
                home.join(format!(".claude/sessions/{pid}.json")),
                format!(r#"{{"sessionId":"s-{pid}","cwd":"/work"}}"#),
            )
            .unwrap();
        }
        let project = home.join(".claude/projects/-work");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("s-4242.jsonl"), "").unwrap();
        fs::write(project.join("s-4100.jsonl"), "").unwrap();
        let none = || Vec::new();

        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "cwd": "/work", "pane_id": "wX:p1"}),
        );
        let found = discover(&registry, "coord", &proc_root, &[], &none).expect("discovered");
        assert_eq!(found.located.path, project.join("s-4242.jsonl"));
        assert!(
            found.located.evidence.contains("running in its pane wX:p1"),
            "{}",
            found.located.evidence
        );
        assert_eq!(
            found.bind.as_deref(),
            Some("s-4242"),
            "a live process: bind"
        );

        // A record that names a conversation already is read the same way, never rebound.
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "cwd": "/work", "pane_id": "wX:p1",
                "native_session": {"agent": "claude", "value": "s-old"}}),
        );
        let found = discover(&registry, "coord", &proc_root, &[], &none).expect("discovered");
        assert_eq!(found.bind, None);

        // No process in the pane, and no reply ID to confirm a file by: nothing.
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "cwd": "/work", "pane_id": "wX:p5"}),
        );
        assert_eq!(
            discover(
                &registry,
                "coord",
                &proc_root,
                &[home.join(".claude")],
                &none
            ),
            None
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn discovery_by_content_needs_a_delivered_reply_id_and_never_binds() {
        let root = scratch("discover-content");
        let config = root.join("claude");
        let project = config.join("projects/-work");
        fs::create_dir_all(&project).unwrap();
        let older = project.join("s-a.jsonl");
        let newer = project.join("s-b.jsonl");
        fs::write(
            &older,
            entry(
                "<CHAT_REPLY_77>\nDone.\n</CHAT_REPLY_77>",
                "2026-11-02T18:00:00Z",
            ),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&newer, entry("Another session.", "2026-11-02T18:00:01Z")).unwrap();
        let registry = registry_with(
            &root,
            &serde_json::json!({"harness": "claude", "cwd": "/work", "pane_id": "wX:p1"}),
        );
        let proc_root = root.join("proc");
        let configs = [root.join("absent"), config.clone()];

        let found = discover(&registry, "coord", &proc_root, &configs, &|| {
            vec!["5".to_owned(), "77".to_owned()]
        })
        .expect("confirmed by its content");
        assert_eq!(found.located.path, older, "the newest file holding the ID");
        assert!(found.located.evidence.contains("reply ID 77"));
        assert_eq!(found.bind, None, "content alone never binds");
        for expected in [vec![], vec!["7".to_owned()]] {
            assert_eq!(
                discover(&registry, "coord", &proc_root, &configs, &|| expected
                    .clone()),
                None,
                "{expected:?}"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_discovered_transcript_is_read_and_shared_as_discovered() {
        let root = scratch("discover-read");
        let path = root.join("s-live.jsonl");
        fs::write(
            &path,
            entry(
                "<CHAT_REPLY_031>\n| a | b |\n</CHAT_REPLY_031>",
                "2026-11-02T18:00:20Z",
            ),
        )
        .unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&logs);
        let shared: SharedReplySource = Arc::default();
        let discovered = path.clone();
        let mut source = CoordinatorTranscript::new(
            Box::new(|| None),
            Box::new(move |line| log.lock().unwrap().push(line)),
        )
        .with_discovery(
            Box::new(move || {
                Some(Discovered {
                    harness: "claude".to_owned(),
                    located: Located {
                        path: discovered.clone(),
                        evidence: "process 4242 names it".to_owned(),
                    },
                    bind: Some("s-live".to_owned()),
                })
            }),
            Arc::clone(&shared),
        );
        let blocks = source.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].body, "| a | b |");
        assert_eq!(
            shared.lock().unwrap().clone(),
            Some(ReplySource {
                path: Some(path),
                evidence: "process 4242 names it".to_owned(),
                discovered: true,
                unbound_session: Some("s-live".to_owned()),
                session_history: false,
                headless_log: false,
            })
        );
        assert!(logs.lock().unwrap()[0].contains("discovered"));

        // Nothing found at all is shared too, with no path.
        let shared: SharedReplySource = Arc::default();
        let mut source = CoordinatorTranscript::new(Box::new(|| None), Box::new(|_| {}))
            .with_discovery(Box::new(|| None), Arc::clone(&shared));
        assert!(source.blocks().is_empty());
        let none = shared.lock().unwrap().clone().expect("shared");
        assert_eq!(none.path, None);
        assert!(!none.discovered);
        fs::remove_dir_all(root).ok();
    }
}

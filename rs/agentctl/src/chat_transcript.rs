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
}

/// The [`ReplySource`] the transcript reader shares with the owner loop: `None` until the first
/// look.
pub(crate) type SharedReplySource = Arc<Mutex<Option<ReplySource>>>;

/// The coordinator's transcript, followed for its reply blocks.
pub(crate) struct CoordinatorTranscript {
    resolve: Resolve,
    discover: Option<Discover>,
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
        }
    }

    /// Follow the transcript of agent `name` in `registry`, as its record describes it, or as
    /// [`discover`] finds it, confirmed by content against the reply IDs `expected` gives;
    /// sharing where the text comes from through `source`.
    pub(crate) fn for_agent(
        registry: PathBuf,
        name: String,
        log: Log,
        expected: Expected,
        source: SharedReplySource,
    ) -> Self {
        let resolve = {
            let (registry, name) = (registry.clone(), name.clone());
            Box::new(move || record_hints(&registry, &name)) as Resolve
        };
        let mut transcript = Self::new(resolve, log);
        transcript.discover = Some(Box::new(move || {
            discover(
                &registry,
                &name,
                Path::new("/proc"),
                &crate::harness::claude::config_roots(),
                &expected,
            )
        }));
        transcript.source = Some(source);
        transcript
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
        let mut source = Self::new(Box::new(move || record_hints(&registry, &name)), log);
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

    /// Look for the transcript again when it is due, switching to another file if it moved.
    fn relocate(&mut self) {
        // Looking reads the process table, so it is done at most once per period, found or not.
        if self
            .located_at
            .is_some_and(|at| at.elapsed() < RELOCATE_EVERY)
        {
            return;
        }
        self.located_at = Some(Instant::now());
        let found = (self.resolve)()
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
            self.share(ReplySource {
                evidence: "no transcript of the coordinator was found: its record names no \
                           conversation or process that finds one, and none was discovered"
                    .to_owned(),
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

    /// Read what the transcript gained, keeping its complete reply blocks.
    fn poll(&mut self) {
        self.relocate();
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        let events = match follower.poll() {
            Ok(events) => events,
            Err(error) => {
                let path = follower.path().display().to_string();
                self.follower = None;
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
        };
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
            if self.follower.is_none()
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
        self.follower.is_some()
    }
}

/// The harness and transcript hints of agent `name` in `registry`, from its record: the pinned
/// harness process, its working directory, and the conversation id it records, if any.
fn record_hints(registry: &Path, name: &str) -> Option<(String, Hints)> {
    let path = agent_queue::registry::directory(registry, name)
        .ok()?
        .join("agent.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let harness = record["harness"].as_str()?.to_owned();
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

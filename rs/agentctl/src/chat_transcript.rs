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

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_harness::{replies, Event, Follower, Hints};

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

/// What finds the coordinator now: its harness's name and what locates its transcript.
pub(crate) type Resolve = Box<dyn Fn() -> Option<(String, Hints)> + Send>;
/// Where log lines go.
pub(crate) type Log = Box<dyn Fn(String) + Send>;

/// The coordinator's transcript, followed for its reply blocks.
pub(crate) struct CoordinatorTranscript {
    resolve: Resolve,
    log: Log,
    follower: Option<Follower>,
    located_at: Option<Instant>,
    /// The last thing logged about what is read, so each change is logged once.
    reported: Option<String>,
    blocks: VecDeque<TranscriptBlock>,
    bytes: usize,
    waited: VecDeque<String>,
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
            log,
            follower: None,
            located_at: None,
            reported: None,
            blocks: VecDeque::new(),
            bytes: 0,
            waited: VecDeque::new(),
        }
    }

    /// Follow the transcript of agent `name` in `registry`, as its record describes it.
    pub(crate) fn for_agent(registry: PathBuf, name: String, log: Log) -> Self {
        Self::new(Box::new(move || record_hints(&registry, &name)), log)
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
        let found = (self.resolve)().and_then(|(harness, hints)| {
            let reader = agent_harness::transcript(&harness)?;
            Some((reader, reader.locate(&hints)?))
        });
        let Some((reader, found)) = found else {
            self.follower = None;
            self.report(
                "agentctl: no transcript of the coordinator was found; replies keep the text read \
                 from its screen"
                    .to_owned(),
            );
            return;
        };
        if self
            .follower
            .as_ref()
            .is_some_and(|follower| follower.path() == found.path)
        {
            return;
        }
        self.follower = Some(Follower::open(
            &found.path,
            reader.decoder(),
            FIRST_READ_BYTES,
        ));
        self.report(format!(
            "agentctl: reply text comes from the coordinator's {} transcript {} ({})",
            reader.name(),
            found.path.display(),
            found.evidence
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
                    "agentctl: the coordinator's transcript {path} cannot be read ({error}); \
                     replies keep the text read from its screen"
                ));
                return;
            }
        };
        for event in events {
            let Event::Text { at, text } = event else {
                continue;
            };
            for block in replies::complete_blocks(&text) {
                self.bytes += block.body.len();
                self.blocks.push_back(TranscriptBlock {
                    identifier: block.identifier,
                    body: block.body,
                    at_millis: at.filter(|at| *at >= 0.0).map(|at| (at * 1000.0) as u64),
                });
            }
        }
        while self.blocks.len() > MAX_BLOCKS || self.bytes > MAX_BLOCK_BYTES {
            let Some(oldest) = self.blocks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(oldest.body.len());
        }
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
}

/// The harness and transcript hints of agent `name` in `registry`, from its record: the pinned
/// harness process, its working directory, and the conversation id it records, if any.
fn record_hints(registry: &Path, name: &str) -> Option<(String, Hints)> {
    let path = agent_queue::registry::directory(registry, name)
        .ok()?
        .join("agent.json");
    let record: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let harness = record["harness"].as_str()?.to_owned();
    let session = ["native_session", "session_value", "resume"]
        .iter()
        .find_map(|key| record[*key].as_str().filter(|value| !value.is_empty()))
        .map(str::to_owned);
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
            r#"{"name":"coord","harness":"muse","cwd":"/work","native_session":null,"session_value":"s-9","harness_identity":{"pid":4242}}"#,
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
}

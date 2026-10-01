//! Claude Code's own record of whether the Claude Code session in a Herdr pane is busy.
//!
//! Each running Claude Code process keeps a small JSON record at `<config>/sessions/<pid>.json`,
//! where `<config>` is `$CLAUDE_CONFIG_DIR` or `~/.claude`. Four of its fields are read: `pid`;
//! `procStart`, the process start time in clock ticks after boot, as field 22 of
//! `/proc/<pid>/stat` shows it; `status`; and `statusUpdatedAt`, the Unix time in milliseconds
//! when `status` was last written. Claude Code 2.1.285 writes `idle` only while the session waits
//! at its prompt with nothing it started still running. It writes `busy` during a turn or while a
//! subagent it started runs, `shell` while a command it started in the background runs, and
//! `waiting` while it waits for its user to answer a question or grant a permission. Every other
//! field is ignored and never logged, and only names of the form `<pid>.json` are opened, so the
//! other files in that directory are never read.
//!
//! A record outlives a process that did not exit cleanly, and Linux reuses process ids, so a record
//! counts only while a live process has the same pid and the same start time, checked again after
//! the read. A process is joined to its pane through `HERDR_PANE_ID` in `/proc/<pid>/environ`.
//! Every process started in a pane inherits that variable, including a second Claude Code started
//! by a tool inside the pane's session, so the earliest-started process with a matching record
//! decides. Herdr's own busy or idle state is not used for Claude Code panes because its screen
//! rules can classify a working session as idle
//! (https://github.com/rrnewton/agent-utils/issues/179).
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::client::{process_start_ticks, PROC_STAT_BYTES};

/// Most entries examined in the sessions directory. Claude Code keeps a few small files for each
/// running session, so a directory larger than this is not one this reader understands.
const MAX_SESSION_DIRECTORY_ENTRIES: usize = 16_384;
/// Largest session record read; Claude Code writes well under 1 KiB.
const MAX_SESSION_RECORD_BYTES: u64 = 64 * 1024;
/// Most bytes of a process environment searched for `HERDR_PANE_ID`.
const MAX_ENVIRON_BYTES: u64 = 4 * 1024 * 1024;

/// Where Claude Code session records and process information are read from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionSources {
    /// Directory holding the `<pid>.json` session records.
    pub(crate) sessions: PathBuf,
    /// Process filesystem, normally `/proc`.
    pub(crate) proc_root: PathBuf,
}

impl SessionSources {
    /// The sessions directory Claude Code uses in this environment, `$CLAUDE_CONFIG_DIR/sessions`
    /// or else `$HOME/.claude/sessions`, read with `/proc`. `None` when the variable that applies
    /// does not name an absolute directory.
    pub(crate) fn from_environment() -> Option<Self> {
        Self::from_variables(
            std::env::var_os("CLAUDE_CONFIG_DIR"),
            std::env::var_os("HOME"),
        )
    }

    fn from_variables(config: Option<OsString>, home: Option<OsString>) -> Option<Self> {
        let absolute =
            |value: OsString| Some(PathBuf::from(value)).filter(|path| path.is_absolute());
        let config = match config.filter(|value| !value.is_empty()) {
            Some(config) => absolute(config)?,
            None => absolute(home.filter(|value| !value.is_empty())?)?.join(".claude"),
        };
        Some(Self {
            sessions: config.join("sessions"),
            proc_root: PathBuf::from("/proc"),
        })
    }
}

/// One Claude Code session's busy or idle state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeSession {
    /// Process id of the Claude Code process.
    pub(crate) pid: u32,
    /// `idle` only while the session waits at its prompt with nothing it started still running:
    /// see the module documentation. Any value is kept as written.
    pub(crate) status: String,
    /// Unix time in milliseconds when `status` was last written.
    pub(crate) status_updated_at_millis: u64,
}

impl ClaudeSession {
    /// When the session last became idle, if it is idle now.
    pub(crate) fn idle_since_millis(&self) -> Option<u64> {
        (self.status == "idle").then_some(self.status_updated_at_millis)
    }
}

/// The Claude Code session found for a pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PaneSession {
    /// A live process in the pane has a record that matches it.
    Found(ClaudeSession),
    /// No live process in the pane has a session record.
    NotFound,
    /// A record of a live process in the pane cannot be used; the text says why.
    Invalid(String),
}

/// Why one record was not used.
enum Rejected {
    /// The record is not the live process's own record; a later candidate may still match.
    Stale(String),
    /// The record cannot be read as a session record; the lookup stops, so that a later-started
    /// session never stands in for the one whose record is unreadable.
    Unusable(String),
}

/// The fields read from a session record. Each is kept as raw JSON so that a value of an
/// unexpected type is reported here rather than quoted in a parser message.
#[derive(Deserialize)]
struct SessionRecord {
    pid: Option<Value>,
    #[serde(rename = "procStart")]
    proc_start: Option<Value>,
    status: Option<Value>,
    #[serde(rename = "statusUpdatedAt")]
    status_updated_at: Option<Value>,
}

/// The Claude Code session running in Herdr pane `pane_id`.
pub(crate) fn session_for_pane(sources: &SessionSources, pane_id: &str) -> PaneSession {
    session_for_pane_with_hook(sources, pane_id, |_| {})
}

/// [`session_for_pane`], calling `after_read` with the pid of each record it has read and found
/// to match, just before it checks again that the process is live.
fn session_for_pane_with_hook(
    sources: &SessionSources,
    pane_id: &str,
    mut after_read: impl FnMut(u32),
) -> PaneSession {
    let directory = sources.sessions.display();
    let entries = match fs::read_dir(&sources.sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return PaneSession::NotFound,
        Err(error) => {
            return PaneSession::Invalid(format!(
                "cannot list Claude Code sessions in {directory}: {error}"
            ))
        }
    };
    let mut candidates = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == MAX_SESSION_DIRECTORY_ENTRIES {
            return PaneSession::Invalid(format!(
                "{directory} has more than {MAX_SESSION_DIRECTORY_ENTRIES} entries"
            ));
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return PaneSession::Invalid(format!(
                    "cannot list Claude Code sessions in {directory}: {error}"
                ))
            }
        };
        let Some(pid) = record_pid(&entry.file_name()) else {
            continue;
        };
        let Some(start) = live_start(&sources.proc_root, pid) else {
            continue;
        };
        if runs_in_pane(&sources.proc_root, pid, pane_id) {
            candidates.push((start, pid));
        }
    }
    candidates.sort_unstable();
    let mut stale = None;
    for (start, pid) in candidates {
        match read_session(sources, pid, start, &mut after_read) {
            Ok(session) => return PaneSession::Found(session),
            Err(Rejected::Stale(reason)) => {
                stale.get_or_insert(reason);
            }
            Err(Rejected::Unusable(reason)) => return PaneSession::Invalid(reason),
        }
    }
    stale.map_or(PaneSession::NotFound, PaneSession::Invalid)
}

/// The pid named by a session record's file name: decimal digits without a leading zero, then
/// `.json`.
fn record_pid(name: &OsStr) -> Option<u32> {
    let digits = name.as_bytes().strip_suffix(b".json")?;
    if digits.first().is_none_or(|first| *first == b'0') || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Start time of process `pid` while it is live, from `<proc_root>/<pid>/stat`.
fn live_start(proc_root: &Path, pid: u32) -> Option<u64> {
    let path = proc_root.join(pid.to_string()).join("stat");
    let stat = read_bounded(File::open(path).ok()?, PROC_STAT_BYTES as u64).ok()?;
    process_start_ticks(&stat, u64::from(pid))
}

/// Whether process `pid` was started in Herdr pane `pane_id`: the first `HERDR_PANE_ID` in its
/// environment names that pane.
fn runs_in_pane(proc_root: &Path, pid: u32, pane_id: &str) -> bool {
    let path = proc_root.join(pid.to_string()).join("environ");
    let Some(environ) = File::open(path)
        .ok()
        .and_then(|file| read_bounded(file, MAX_ENVIRON_BYTES).ok())
    else {
        return false;
    };
    environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"HERDR_PANE_ID="))
        .is_some_and(|value| value == pane_id.as_bytes())
}

/// Everything `file` holds, or an error when that is more than `limit` bytes.
fn read_bounded(file: File, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("larger than {limit} bytes"),
        ));
    }
    Ok(bytes)
}

/// The record `<pid>.json`, if it belongs to live process `pid`, which started at `start`.
/// `after_read` is called once the record matches, before the process is checked again.
fn read_session(
    sources: &SessionSources,
    pid: u32,
    start: u64,
    after_read: &mut impl FnMut(u32),
) -> Result<ClaudeSession, Rejected> {
    let path = sources.sessions.join(format!("{pid}.json"));
    let record = format!("Claude Code session record {}", path.display());
    let unusable = |detail: &str| Rejected::Unusable(format!("{record} {detail}"));
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(Rejected::Stale(format!(
                "{record} was removed while it was read"
            )))
        }
        Err(error) => return Err(unusable(&format!("cannot be opened: {error}"))),
    };
    let metadata = file
        .metadata()
        .map_err(|error| unusable(&format!("cannot be examined: {error}")))?;
    if !metadata.file_type().is_file() {
        return Err(unusable("is not a regular file"));
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(unusable(&format!(
            "is owned by uid {}, not by this user",
            metadata.uid()
        )));
    }
    let bytes = read_bounded(file, MAX_SESSION_RECORD_BYTES)
        .map_err(|error| unusable(&format!("cannot be read: {error}")))?;
    let fields: SessionRecord = serde_json::from_slice(&bytes).map_err(|error| {
        unusable(&format!(
            "does not parse as a session record ({:?} error at line {}, column {})",
            error.classify(),
            error.line(),
            error.column()
        ))
    })?;
    if fields.pid.as_ref().and_then(decimal) != Some(u64::from(pid)) {
        return Err(Rejected::Stale(format!(
            "{record} does not name process {pid}"
        )));
    }
    match fields.proc_start.as_ref().and_then(decimal) {
        Some(recorded) if recorded == start => {}
        Some(_) => {
            return Err(Rejected::Stale(format!(
                "{record} belongs to an earlier process with the same pid"
            )))
        }
        None => {
            return Err(Rejected::Stale(format!(
                "{record} has no procStart start time to match to process {pid}"
            )))
        }
    }
    let Some(Value::String(status)) = fields.status else {
        return Err(unusable("has no status text"));
    };
    let Some(status_updated_at_millis) = fields.status_updated_at.as_ref().and_then(decimal) else {
        return Err(unusable("has no statusUpdatedAt time"));
    };
    after_read(pid);
    if live_start(&sources.proc_root, pid) != Some(start) {
        return Err(Rejected::Stale(format!(
            "process {pid} exited while {record} was read"
        )));
    }
    Ok(ClaudeSession {
        pid,
        status,
        status_updated_at_millis,
    })
}

/// A non-negative integer written as a JSON number or as a string of decimal digits.
fn decimal(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text)
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse().ok()
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PANE: &str = "w1:p2";

    fn temporary(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "agentctl-claude-session-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("temporary directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private temporary");
        path
    }

    /// A fake `/proc` and sessions directory under one temporary directory.
    pub(crate) struct Fixture {
        root: PathBuf,
        pub(crate) sources: SessionSources,
    }

    impl Fixture {
        pub(crate) fn new(name: &str) -> Self {
            let root = temporary(name);
            let sources = SessionSources {
                sessions: root.join("sessions"),
                proc_root: root.join("proc"),
            };
            fs::create_dir(&sources.sessions).unwrap();
            fs::create_dir(&sources.proc_root).unwrap();
            Self { root, sources }
        }

        /// A process `pid` in state `state`, started at `start`, whose environment names `pane`.
        pub(crate) fn process(&self, pid: u32, comm: &str, state: char, start: u64, pane: &str) {
            let directory = self.sources.proc_root.join(pid.to_string());
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("stat"),
                format!(
                    "{pid} ({comm}) {state} 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 \
                     {start} 0 0\n"
                ),
            )
            .unwrap();
            fs::write(
                directory.join("environ"),
                format!("PATH=/bin\0HERDR_PANE_ID={pane}\0HERDR_PANE_ID=other\0TERM=xterm\0"),
            )
            .unwrap();
        }

        fn record(&self, name: &str, text: &str) {
            fs::write(self.sources.sessions.join(name), text).unwrap();
        }

        /// The record a live Claude Code process `pid` started at `start` writes.
        pub(crate) fn session(&self, pid: u32, start: u64, status: &str, updated: u64) {
            self.record(
                &format!("{pid}.json"),
                &session_text(pid, start, status, updated),
            );
        }

        fn lookup(&self) -> PaneSession {
            session_for_pane(&self.sources, PANE)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// The text of the record a live Claude Code process `pid` started at `start` writes.
    pub(crate) fn session_text(pid: u32, start: u64, status: &str, updated: u64) -> String {
        format!(
            r#"{{"pid":{pid},"sessionId":"00000000-0000-4000-8000-000000000000","cwd":"/work","procStart":"{start}","status":"{status}","statusUpdatedAt":{updated},"kind":"interactive"}}"#
        )
    }

    fn found(pid: u32, status: &str, updated: u64) -> PaneSession {
        PaneSession::Found(ClaudeSession {
            pid,
            status: status.to_owned(),
            status_updated_at_millis: updated,
        })
    }

    fn invalid_reason(session: PaneSession) -> String {
        match session {
            PaneSession::Invalid(reason) => reason,
            other => panic!("expected an invalid session, got {other:?}"),
        }
    }

    #[test]
    fn a_live_session_in_the_pane_is_found_and_another_pane_is_ignored() {
        let fixture = Fixture::new("found");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 1_700_000_000_000);
        fixture.process(77, "claude", 'S', 100, "w1:p3");
        fixture.session(77, 100, "busy", 5);
        let session = fixture.lookup();
        assert_eq!(session, found(4242, "idle", 1_700_000_000_000));
        let PaneSession::Found(session) = session else {
            unreachable!()
        };
        assert_eq!(session.idle_since_millis(), Some(1_700_000_000_000));
    }

    #[test]
    fn a_busy_or_unknown_status_is_never_idle() {
        let fixture = Fixture::new("busy");
        fixture.process(4242, "claude", 'R', 900, PANE);
        fixture.session(4242, 900, "busy", 12);
        let PaneSession::Found(session) = fixture.lookup() else {
            panic!("expected a session")
        };
        assert_eq!(session.idle_since_millis(), None);
        let waiting = ClaudeSession {
            status: "waiting".to_owned(),
            ..session
        };
        assert_eq!(waiting.idle_since_millis(), None);
    }

    #[test]
    fn a_reused_pid_with_another_start_time_is_rejected() {
        let fixture = Fixture::new("reused");
        fixture.process(4242, "bash", 'S', 901, PANE);
        fixture.session(4242, 900, "idle", 12);
        let reason = invalid_reason(fixture.lookup());
        assert!(
            reason.contains("earlier process with the same pid"),
            "{reason}"
        );
    }

    #[test]
    fn a_process_that_exits_or_is_replaced_while_its_record_is_read_is_not_taken() {
        // The record is read after the process's start time, so once the record matches, the
        // process is checked again: it may have exited in between, or another process may have
        // taken its pid.
        for replaced in [false, true] {
            let fixture = Fixture::new("gone-after-read");
            fixture.process(4242, "claude", 'S', 900, PANE);
            fixture.session(4242, 900, "idle", 12);
            let mut checked = Vec::new();
            let session = session_for_pane_with_hook(&fixture.sources, PANE, |pid| {
                checked.push(pid);
                if replaced {
                    fixture.process(4242, "bash", 'S', 901, PANE);
                } else {
                    fs::remove_dir_all(fixture.sources.proc_root.join("4242")).unwrap();
                }
            });
            assert_eq!(checked, [4242]);
            let reason = invalid_reason(session);
            assert!(reason.contains("process 4242 exited while"), "{reason}");
        }
    }

    #[test]
    fn a_record_that_names_another_pid_is_rejected() {
        let fixture = Fixture::new("other-pid");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.record("4242.json", &session_text(4243, 900, "idle", 12));
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("does not name process 4242"), "{reason}");
    }

    #[test]
    fn a_record_without_a_start_time_is_rejected() {
        let fixture = Fixture::new("no-start");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.record(
            "4242.json",
            r#"{"pid":4242,"status":"idle","statusUpdatedAt":12}"#,
        );
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("has no procStart"), "{reason}");
    }

    #[test]
    fn nothing_is_found_without_a_session_in_the_pane() {
        let fixture = Fixture::new("not-found");
        assert_eq!(fixture.lookup(), PaneSession::NotFound);
        fixture.process(4242, "claude", 'S', 900, "w1:p3");
        fixture.session(4242, 900, "idle", 12);
        assert_eq!(fixture.lookup(), PaneSession::NotFound);
        // A process in the pane without a record, and a record without a process.
        fixture.process(4243, "bash", 'S', 800, PANE);
        fixture.session(4244, 950, "idle", 12);
        assert_eq!(fixture.lookup(), PaneSession::NotFound);
        let missing = SessionSources {
            sessions: fixture.root.join("absent"),
            proc_root: fixture.sources.proc_root.clone(),
        };
        assert_eq!(session_for_pane(&missing, PANE), PaneSession::NotFound);
    }

    #[test]
    fn a_damaged_record_of_the_pane_session_is_invalid() {
        let damaged = [
            ("", "does not parse"),
            (
                "{\"pid\":4242,\"procStart\":\"900\",\"sta",
                "does not parse",
            ),
            ("[]", "does not parse"),
            ("\"idle\"", "does not parse"),
            (
                r#"{"pid":4242,"procStart":"900","statusUpdatedAt":12}"#,
                "has no status",
            ),
            (
                r#"{"pid":4242,"procStart":"900","status":7,"statusUpdatedAt":12}"#,
                "has no status",
            ),
            (
                r#"{"pid":4242,"procStart":"900","status":"idle"}"#,
                "has no statusUpdatedAt",
            ),
            (
                r#"{"pid":4242,"procStart":"900","status":"idle","statusUpdatedAt":-1}"#,
                "has no statusUpdatedAt",
            ),
            (
                r#"{"pid":4242,"procStart":"900","status":"idle","statusUpdatedAt":"soon"}"#,
                "has no statusUpdatedAt",
            ),
        ];
        for (text, expected) in damaged {
            let fixture = Fixture::new("damaged");
            fixture.process(4242, "claude", 'S', 900, PANE);
            fixture.record("4242.json", text);
            let reason = invalid_reason(fixture.lookup());
            assert!(reason.contains(expected), "{text:?}: {reason}");
            assert!(
                !reason.contains("soon") && !reason.contains("idle"),
                "{reason}"
            );
        }
    }

    #[test]
    fn the_earliest_started_session_in_the_pane_decides() {
        let fixture = Fixture::new("earliest");
        fixture.process(9000, "claude", 'S', 100, PANE);
        fixture.session(9000, 100, "busy", 50);
        fixture.process(12, "claude", 'S', 400, PANE);
        fixture.session(12, 400, "idle", 60);
        assert_eq!(fixture.lookup(), found(9000, "busy", 50));
    }

    #[test]
    fn an_unreadable_record_of_the_earliest_session_is_not_replaced_by_a_later_one() {
        let fixture = Fixture::new("no-fallthrough");
        fixture.process(9000, "claude", 'S', 100, PANE);
        fixture.record("9000.json", "{\"pid\":9000,\"procS");
        fixture.process(12, "claude", 'S', 400, PANE);
        fixture.session(12, 400, "idle", 60);
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("9000.json"), "{reason}");
    }

    #[test]
    fn a_stale_record_of_an_earlier_process_in_the_pane_is_skipped() {
        let fixture = Fixture::new("stale-skip");
        // The pane's shell reused the pid of a Claude Code process that did not exit cleanly.
        fixture.process(300, "bash", 'S', 50, PANE);
        fixture.session(300, 20, "busy", 1);
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 70);
        assert_eq!(fixture.lookup(), found(4242, "idle", 70));
    }

    #[test]
    fn a_record_of_a_zombie_or_exited_process_is_ignored() {
        let fixture = Fixture::new("exited");
        for (pid, state) in [(500, 'Z'), (501, 'X'), (502, 'x')] {
            fixture.process(pid, "claude", state, 100, PANE);
            fixture.session(pid, 100, "idle", 1);
        }
        fixture.session(503, 100, "idle", 1);
        assert_eq!(fixture.lookup(), PaneSession::NotFound);
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 70);
        assert_eq!(fixture.lookup(), found(4242, "idle", 70));
    }

    #[test]
    fn only_canonical_pid_record_names_are_opened() {
        assert_eq!(record_pid(OsStr::new("4242.json")), Some(4242));
        assert_eq!(record_pid(OsStr::new("1.json")), Some(1));
        assert_eq!(record_pid(OsStr::new("4294967295.json")), Some(u32::MAX));
        for name in [
            "4242.key",
            ".4242.json",
            "04242.json",
            "0.json",
            ".json",
            "4242.json.tmp",
            "4242.JSON",
            "4294967296.json",
            "+42.json",
            "-42.json",
            "42 .json",
            "４２.json",
        ] {
            assert_eq!(record_pid(OsStr::new(name)), None, "{name}");
        }
    }

    #[test]
    fn other_files_in_the_sessions_directory_are_not_read() {
        let fixture = Fixture::new("other-files");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 70);
        fixture.process(7, "claude", 'S', 1, PANE);
        // Neither name is a canonical record name, so neither is opened.
        fixture.record("4242.key", "not json");
        fixture.record("07.json", "not json");
        fs::create_dir(fixture.sources.sessions.join("7.json.d")).unwrap();
        assert_eq!(fixture.lookup(), found(4242, "idle", 70));
    }

    #[test]
    fn a_record_that_is_not_a_regular_file_is_invalid_without_blocking() {
        let fixture = Fixture::new("fifo");
        fixture.process(4242, "claude", 'S', 900, PANE);
        let path = CString::new(
            fixture
                .sources
                .sessions
                .join("4242.json")
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("is not a regular file"), "{reason}");
    }

    #[test]
    fn a_symbolic_link_record_is_not_followed() {
        let fixture = Fixture::new("symlink");
        fixture.process(4242, "claude", 'S', 900, PANE);
        let target = fixture.root.join("elsewhere.json");
        fs::write(&target, session_text(4242, 900, "idle", 70)).unwrap();
        std::os::unix::fs::symlink(&target, fixture.sources.sessions.join("4242.json")).unwrap();
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("cannot be opened"), "{reason}");
    }

    #[test]
    fn an_oversized_record_is_invalid() {
        let fixture = Fixture::new("oversized");
        fixture.process(4242, "claude", 'S', 900, PANE);
        let padding = " ".repeat(MAX_SESSION_RECORD_BYTES as usize);
        fixture.record(
            "4242.json",
            &format!("{}{padding}", session_text(4242, 900, "idle", 70)),
        );
        let reason = invalid_reason(fixture.lookup());
        assert!(reason.contains("larger than"), "{reason}");
    }

    #[test]
    fn numbers_may_be_written_as_numbers_or_digit_strings() {
        let fixture = Fixture::new("number-forms");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.record(
            "4242.json",
            r#"{"pid":"4242","procStart":900,"status":"idle","statusUpdatedAt":"70"}"#,
        );
        assert_eq!(fixture.lookup(), found(4242, "idle", 70));
    }

    #[test]
    fn a_command_name_with_brackets_and_spaces_does_not_confuse_the_start_time() {
        let fixture = Fixture::new("comm");
        fixture.process(4242, "a) (b) S 1 2", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 70);
        assert_eq!(fixture.lookup(), found(4242, "idle", 70));
    }

    #[test]
    fn the_pane_is_named_by_the_first_herdr_pane_id_only() {
        let fixture = Fixture::new("first-pane-id");
        fixture.process(4242, "claude", 'S', 900, PANE);
        fixture.session(4242, 900, "idle", 70);
        // The fixture environment also holds a second HERDR_PANE_ID=other.
        assert_eq!(
            session_for_pane(&fixture.sources, "other"),
            PaneSession::NotFound
        );
        assert_eq!(
            session_for_pane(&fixture.sources, "w1:p"),
            PaneSession::NotFound
        );
    }

    #[test]
    fn the_sessions_directory_follows_claude_config_dir_then_home() {
        let sessions = |config: Option<&str>, home: Option<&str>| {
            SessionSources::from_variables(config.map(OsString::from), home.map(OsString::from))
                .map(|sources| (sources.sessions, sources.proc_root))
        };
        let expect = |path: &str| Some((PathBuf::from(path), PathBuf::from("/proc")));
        assert_eq!(sessions(Some("/c"), Some("/h")), expect("/c/sessions"));
        assert_eq!(sessions(None, Some("/h")), expect("/h/.claude/sessions"));
        assert_eq!(
            sessions(Some(""), Some("/h")),
            expect("/h/.claude/sessions")
        );
        assert_eq!(sessions(Some("relative"), Some("/h")), None);
        assert_eq!(sessions(None, Some("relative")), None);
        assert_eq!(sessions(None, Some("")), None);
        assert_eq!(sessions(None, None), None);
    }
}

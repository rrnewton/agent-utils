//! What an agent is doing, from its harness's own transcript rather than its screen (#574
//! harness-adapters).
//!
//! The screen says a turn runs and how long; it cannot say whether the model is streaming a reply,
//! waiting on a model request that never answers, or waiting on one tool call, nor whether that
//! call set a timeout. Each harness appends its conversation to a JSONL file as it goes, and that
//! file can. This module holds what is common to every harness: how the file is found (the
//! record's session id, or the files the harness process holds open), how its tail is read, and
//! the [`Activity`] the per-harness readers fill in. Each adapter supplies its own location and
//! entry format.
//!
//! Only the tail of a transcript is read ([`TAIL_BYTES`]): Muse transcripts pass 100 MB in a day.
//! A tool call whose start has scrolled out of the tail is not seen; its result, if it arrives,
//! is ignored.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::HarnessAdapter;
use crate::work_state::ProcView;

/// Most bytes read from the end of a transcript.
pub const TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// Characters kept of a tool call's command.
const COMMAND_CHARS: usize = 200;

/// What locates one agent's transcript.
pub struct Hints<'a> {
    /// The native conversation id the record holds, if any.
    pub session: Option<&'a str>,
    /// The agent's working directory.
    pub cwd: Option<&'a Path>,
    /// The home directory the harness runs under.
    pub home: Option<&'a Path>,
    /// The pinned harness process.
    pub pid: Option<u32>,
    /// Where process facts come from.
    pub proc: Option<&'a dyn ProcView>,
}

impl Hints<'_> {
    /// A variable from the harness process's environment, when it can be read.
    #[must_use]
    pub fn environment(&self, key: &str) -> Option<String> {
        let entries = self.proc?.environ(self.pid?)?;
        entries.iter().find_map(|entry| {
            entry
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    }

    /// Every regular file the harness process and its descendants hold open, as paths, with the
    /// kernel's ` (deleted)` suffix removed.
    #[must_use]
    pub fn open_files(&self) -> Vec<PathBuf> {
        let (Some(proc), Some(root)) = (self.proc, self.pid) else {
            return Vec::new();
        };
        let processes = proc.processes();
        let mut pids = vec![root];
        let mut index = 0;
        while index < pids.len() {
            let parent = pids[index];
            pids.extend(
                processes
                    .iter()
                    .filter(|row| row.ppid == parent && row.pid != parent)
                    .map(|row| row.pid),
            );
            index += 1;
        }
        let mut files = Vec::new();
        for pid in pids {
            for fd in proc.fds(pid) {
                if !fd.target.starts_with('/') {
                    continue;
                }
                let target = fd.target.strip_suffix(" (deleted)").unwrap_or(&fd.target);
                let path = PathBuf::from(target);
                if !files.contains(&path) {
                    files.push(path);
                }
            }
        }
        files
    }
}

/// What bounds a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "seconds", rename_all = "kebab-case")]
pub enum ToolLimit {
    /// The call set a timeout (or a yield time after which it returns) of this many seconds.
    Explicit(f64),
    /// The call set none, and the harness bounds it with this default.
    HarnessDefault(f64),
    /// The call runs in the background and returned at once.
    Background,
    /// Nothing bounds the call.
    None,
}

/// One tool call the transcript shows started and not finished.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCall {
    /// The harness's call id.
    #[serde(skip)]
    pub id: String,
    /// Tool name (`Bash`, `exec`, `bash`, ...).
    pub name: String,
    /// The command or the most telling argument, bounded.
    pub command: Option<String>,
    /// Unix seconds when the call was made.
    pub started_at: f64,
    /// What bounds it.
    pub limit: ToolLimit,
}

impl ToolCall {
    /// Whether a call running `elapsed` seconds is unbounded: it set no timeout and the harness
    /// imposes none.
    #[must_use]
    pub fn untimed(&self) -> bool {
        self.limit == ToolLimit::None
    }

    /// One phrase naming the call: `Bash \`make validate\``.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.command {
            Some(command) => format!("{} `{command}`", self.name),
            None => self.name.clone(),
        }
    }

    /// One phrase for its limit: `no timeout`, `timeout 600 s`, ...
    #[must_use]
    pub fn describe_limit(&self) -> String {
        match self.limit {
            ToolLimit::Explicit(seconds) => format!("timeout {} s", seconds.round()),
            ToolLimit::HarnessDefault(seconds) => {
                format!(
                    "no timeout set; the harness default is {} s",
                    seconds.round()
                )
            }
            ToolLimit::Background => "runs in the background".to_owned(),
            ToolLimit::None => "no timeout".to_owned(),
        }
    }
}

/// What a transcript's tail says the agent is doing. Times are Unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Activity {
    /// The transcript read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The newest entry of any kind.
    pub last_event_at: Option<f64>,
    /// Its kind, in the harness's words or ours (`tool result`, `api-request`, ...).
    pub last_event: Option<String>,
    /// The newest model output: reply text, reasoning, a tool call.
    pub last_output_at: Option<f64>,
    /// Its kind.
    pub last_output: Option<String>,
    /// The newest entry that shows the agent moving: output, a request, a prompt, a tool call
    /// starting or finishing. Harness bookkeeping does not count.
    pub last_progress_at: Option<f64>,
    /// When the newest model request was sent, when no output has come back since.
    pub request_since: Option<f64>,
    /// The newest prompt that started a turn.
    pub prompt_at: Option<f64>,
    /// The oldest foreground tool call still in flight.
    pub tool: Option<ToolCall>,
    /// How many tool calls are in flight, background ones included.
    pub open_tools: usize,
    /// Why the transcript could not be read, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Activity {
    /// Note an entry of `kind` at `at` that shows the agent moving.
    pub fn event(&mut self, at: f64, kind: &str) {
        self.bookkeeping(at, kind);
        if self.last_progress_at.is_none_or(|last| at >= last) {
            self.last_progress_at = Some(at);
        }
    }

    /// Note harness bookkeeping: it moves only the last-event clock.
    pub fn bookkeeping(&mut self, at: f64, kind: &str) {
        if self.last_event_at.is_none_or(|last| at >= last) {
            self.last_event_at = Some(at);
            self.last_event = Some(kind.to_owned());
        }
    }

    /// Note model output of `kind` at `at`; it answers any request sent before it.
    pub fn output(&mut self, at: f64, kind: &str) {
        self.event(at, kind);
        if self.last_output_at.is_none_or(|last| at >= last) {
            self.last_output_at = Some(at);
            self.last_output = Some(kind.to_owned());
        }
        if self.request_since.is_some_and(|since| at >= since) {
            self.request_since = None;
        }
    }

    /// Note a model request sent at `at`.
    pub fn request(&mut self, at: f64) {
        self.event(at, "model request");
        if self.last_output_at.is_none_or(|last| at >= last) {
            self.request_since = Some(at);
        }
    }

    /// Note a prompt that starts a turn.
    pub fn prompt(&mut self, at: f64) {
        self.event(at, "prompt");
        self.prompt_at = Some(at);
        self.request_since = None;
    }

    /// Record the calls still in flight at the end of the tail.
    pub fn set_open_tools(&mut self, mut open: Vec<ToolCall>) {
        for call in &mut open {
            if let Some(command) = &call.command {
                let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
                call.command = Some(flat.chars().take(COMMAND_CHARS).collect());
            }
        }
        self.open_tools = open.len();
        self.tool = open
            .into_iter()
            .filter(|call| call.limit != ToolLimit::Background)
            .min_by(|left, right| left.started_at.total_cmp(&right.started_at));
    }

    /// Seconds since the newest model output, as of `now`.
    #[must_use]
    pub fn output_age(&self, now: f64) -> Option<f64> {
        self.last_output_at.map(|at| (now - at).max(0.0))
    }
}

/// Unix seconds of an RFC 3339 UTC timestamp such as `2026-10-27T12:35:26.720Z` (an offset of
/// `+00:00` is accepted too).
#[must_use]
pub fn parse_utc(text: &str) -> Option<f64> {
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0.0
    } else {
        format!("0.{fraction}").parse::<f64>().ok()?
    };
    let days = crate::prompt_time::days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) as f64 + fraction)
}

/// The complete lines of the last `limit` bytes of `path`. A first line cut by the limit is
/// dropped, and so is a last line without its newline (still being written).
pub fn tail_lines(path: &Path, limit: u64) -> std::io::Result<Vec<Vec<u8>>> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    // One byte before the window says whether the window starts a line.
    let from = length.saturating_sub(limit).saturating_sub(1);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(length - from).read_to_end(&mut bytes)?;
    let start = if from == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    };
    let end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1)
        .max(start);
    Ok(bytes[start..end]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

/// Read the transcript `adapter` finds for `hints`. `None` when no transcript could be found;
/// an [`Activity`] with `error` set when one was found and could not be read.
#[must_use]
pub fn read(adapter: &dyn HarnessAdapter, hints: &Hints<'_>) -> Option<Activity> {
    let path = adapter.transcript_path(hints)?;
    let mut activity = Activity {
        path: Some(path.display().to_string()),
        ..Activity::default()
    };
    match tail_lines(&path, TAIL_BYTES) {
        Ok(lines) => {
            let lines: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
            adapter.read_transcript(&lines, &mut activity);
        }
        Err(error) => activity.error = Some(error.to_string()),
    }
    Some(activity)
}

/// The newest of `paths` by modification time.
pub(crate) fn newest(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, path)| path)
}

/// Files named by `matches` in the date-sharded tree `root/YYYY/MM/DD/...` that both Codex and
/// Muse write, newest days first, searching at most `days` day directories.
pub(crate) fn find_in_dated_tree(
    root: &Path,
    days: usize,
    matches: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let sorted = |path: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        entries.reverse();
        entries
    };
    let mut seen = 0;
    for year in sorted(root) {
        for month in sorted(&year) {
            for day in sorted(&month) {
                seen += 1;
                if seen > days {
                    return None;
                }
                for entry in sorted(&day) {
                    if let Some(found) = matches(&entry) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_parse_with_and_without_fractions() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_utc("2026-10-27T12:35:26.500Z"), Some(1_793_104_526.5));
        assert_eq!(
            parse_utc("2026-10-27T12:35:26+00:00"),
            Some(1_793_104_526.0)
        );
        assert_eq!(parse_utc("2026-10-27T12:35:26"), None, "no zone");
        assert_eq!(parse_utc("2026-13-27T12:35:26Z"), None);
    }

    #[test]
    fn a_tail_drops_the_cut_first_line_and_the_unfinished_last_one() {
        let directory = crate::turn_state::tests::scratch();
        let path = directory.join("t.jsonl");
        fs::write(&path, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n{\"d\":").unwrap();
        let all = tail_lines(&path, 1 << 20).unwrap();
        assert_eq!(
            all,
            [
                b"{\"a\":1}".to_vec(),
                b"{\"b\":2}".to_vec(),
                b"{\"c\":3}".to_vec()
            ]
        );
        // The last 13 bytes start exactly at the third line; 12 start inside it.
        assert_eq!(tail_lines(&path, 13).unwrap(), [b"{\"c\":3}".to_vec()]);
        assert!(tail_lines(&path, 12).unwrap().is_empty());
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn output_answers_an_earlier_request_and_a_prompt_starts_afresh() {
        let mut activity = Activity::default();
        activity.output(10.0, "assistant text");
        activity.request(20.0);
        assert_eq!(activity.request_since, Some(20.0));
        activity.output(25.0, "tool call");
        assert_eq!(activity.request_since, None);
        activity.request(30.0);
        activity.prompt(40.0);
        assert_eq!(activity.request_since, None);
        assert_eq!(activity.last_event.as_deref(), Some("prompt"));
        assert_eq!(activity.last_output_at, Some(25.0));
        activity.bookkeeping(50.0, "attachment");
        assert_eq!(activity.last_event_at, Some(50.0));
        assert_eq!(
            activity.last_progress_at,
            Some(40.0),
            "bookkeeping is not progress"
        );
    }
}

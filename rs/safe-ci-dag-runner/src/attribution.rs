//! Evidence that survives the kill: durable per-step logs and test-level culprit attribution.
//!
//! THE PROBLEM THIS SOLVES. Two facts about the runner used to combine badly. Captured step output
//! lived only in memory until the step ended, and the per-step profile rows are written at END OF
//! RUN. So the one mechanism that reliably stops a runaway — a hard kill, whether ours or the CI
//! provider's job cancellation — is also the mechanism that destroys every trace of why it was
//! needed. A cancelled GitHub job discards its logs entirely. That is the single largest reason a
//! slow-step regression stayed unexplained for days: the lane could not report on its own failure.
//!
//! The fix is INCREMENTAL EMISSION. Everything here is appended to disk and flushed AS IT HAPPENS,
//! never accumulated for an end-of-run summary:
//!
//! * a per-step `<tag>.log` receiving the step's interleaved stdout/stderr byte-for-byte, and
//! * a run-level `journal.jsonl` of one-line records: step start, each recognized test boundary,
//!   teardown, and the culprit verdict.
//!
//! Kill the runner with SIGKILL at any instant and the directory still describes what was running.
//!
//! TEST-LEVEL ATTRIBUTION. Naming the NODE that hung ("the strict-compat step") is not enough to
//! act on; the actionable fact is WHICH TEST inside that node. [`TestTracker`] derives it from the
//! step's own output stream as the bytes arrive, tracking the last test STARTED against the last
//! test COMPLETED. On teardown the culprit is the test that started and never completed.
//!
//! THE UNTERMINATED TAIL IS THE STRONGEST SIGNAL, and it is why the reader here splits lines
//! itself instead of using `read_until(b'\n')`. Rust's libtest prints `test some::name ... ` and
//! then RUNS the test, emitting the verdict on the same line afterwards. A line-oriented reader
//! sees that text only once the test finishes — precisely never, for the test that hung. Reading
//! chunks and keeping the bytes since the last newline leaves the hung test's own start marker
//! sitting in [`StepStream::partial`] at the moment of the kill.
//!
//! WHAT IS RECOGNIZED. Rust libtest, pytest (`-v`), TAP, and an explicit `##TEST-START`/`##TEST-END`
//! protocol any harness can adopt. An unrecognized harness degrades to exactly today's behaviour:
//! the node is named and no test is, which is strictly better than the run reporting nothing.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable that overrides the evidence directory.
pub const LOG_DIR_ENV: &str = "SAFE_CI_DAG_RUNNER_LOG_DIR";
/// Set to `1` to disable durable per-step logs and the journal entirely.
pub const NO_LOGS_ENV: &str = "SAFE_CI_DAG_RUNNER_NO_STEP_LOGS";
/// Environment variable carrying the per-step ownership nonce (see `scheduler::kill_by_nonce`).
pub const STEP_NONCE_ENV: &str = "SAFE_CI_DAG_RUNNER_STEP";

/// Monotonic per-process counter making each step's nonce unique within this runner.
static NONCE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Mint a per-step ownership token that every descendant inherits through its environment.
///
/// The token is the runner's pid, a process-start-relative sequence number, and the wall clock in
/// nanoseconds, so it is unique across concurrent steps, across runs, and across pid reuse.
pub fn mint_step_nonce(tag: &str) -> String {
    let seq = NONCE_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // The tag is included for human readability when the token shows up in a `ps` listing; the
    // uniqueness comes entirely from pid/seq/nanos, never from the tag.
    format!("{}:{}:{}:{}", std::process::id(), seq, nanos, sanitize(tag))
}

/// Filesystem-safe rendering of a step tag (used for nonces and log file names).
pub fn sanitize(tag: &str) -> String {
    tag.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Where a run's durable evidence is written, or `None` when the operator opted out.
///
/// DEFAULT ON, and deliberately so. A feature that only records evidence when someone remembered
/// to enable it records nothing on the day it is needed, because a runaway is by definition
/// unanticipated. The default location is under the CI runner's own temp area (`RUNNER_TEMP`) or
/// `TMPDIR`, never the working tree, so a default-on writer cannot dirty a checkout.
pub fn default_log_dir() -> Option<PathBuf> {
    if std::env::var(NO_LOGS_ENV)
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return None;
    }
    if let Ok(dir) = std::env::var(LOG_DIR_ENV) {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let base = std::env::var("RUNNER_TEMP")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("TMPDIR").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/tmp".to_string());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(PathBuf::from(base).join("safe-ci-dag-runner").join(format!(
        "run-{}-{}",
        std::process::id(),
        nanos
    )))
}

/// Seconds since the epoch, as a fixed-precision string for journal records.
fn now_s() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}

/// Minimal JSON string escaping (the journal writes only strings and numbers).
fn jesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The run-level append-only journal: one JSON object per line, flushed on every write.
///
/// Flushing every record is the whole point. A buffered writer would hold the last — and therefore
/// most interesting — records in memory at exactly the moment a SIGKILL discards them.
pub struct RunEvidence {
    dir: PathBuf,
    journal: Mutex<Option<File>>,
}

impl RunEvidence {
    /// Create the evidence directory and open the journal. Returns `None` when logging is disabled
    /// or the directory cannot be created; evidence capture must never be able to fail a run.
    pub fn open(dir: Option<PathBuf>) -> Option<Self> {
        let dir = dir?;
        if std::fs::create_dir_all(&dir).is_err() {
            eprintln!(
                "[scheduler] WARNING: could not create evidence directory {}; per-step logs and \
                 test-level attribution are disabled for this run.",
                dir.display()
            );
            return None;
        }
        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("journal.jsonl"))
            .ok();
        Some(RunEvidence {
            dir,
            journal: Mutex::new(journal),
        })
    }

    /// The directory holding this run's per-step logs and journal.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Append one record and flush it. Fields are emitted in the given order after `ts`/`event`.
    pub fn record(&self, event: &str, fields: &[(&str, String)]) {
        let mut line = format!("{{\"ts\":\"{}\",\"event\":\"{}\"", now_s(), jesc(event));
        for (k, v) in fields {
            line.push_str(&format!(",\"{}\":\"{}\"", jesc(k), jesc(v)));
        }
        line.push_str("}\n");
        if let Ok(mut guard) = self.journal.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
    }

    /// Open (create/truncate) the durable log file for one step.
    pub fn open_step_log(&self, tag: &str) -> Option<File> {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.dir.join(format!("{}.log", sanitize(tag))))
            .ok()
    }
}

/// A recognized test-boundary event in a step's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestEvent {
    /// A named test began running.
    Start(String),
    /// A named test finished, with the harness's verdict token (`ok`, `FAILED`, `PASSED`, ...).
    End(String, String),
}

/// Recognize a test boundary in one line of harness output.
///
/// DELIBERATELY CONSERVATIVE. A false positive renames the culprit and is worse than no name at
/// all, so each pattern is anchored at the start of the line and requires the harness's own
/// punctuation. Anything unrecognized returns `None` and the step simply has no test-level detail.
pub fn recognize(line: &str) -> Option<TestEvent> {
    // NOTE the asymmetry: TRAILING whitespace is preserved for the libtest check below, because
    // `test name ... ` with nothing after the ellipsis is exactly the announcement of a test that
    // has begun but not finished — trimming it away would erase the distinction between "started"
    // and "finished" and with it every hung-test attribution.
    let raw = line.trim_start().trim_end_matches(['\n', '\r']);
    let s = raw.trim_end();

    // 1) Explicit protocol, for any harness that wants exact attribution:
    //      ##TEST-START <name>   ##TEST-END <name> [verdict]
    if let Some(rest) = s.strip_prefix("##TEST-START ") {
        let name = rest.trim();
        if !name.is_empty() {
            return Some(TestEvent::Start(name.to_string()));
        }
    }
    if let Some(rest) = s.strip_prefix("##TEST-END ") {
        let mut it = rest.trim().splitn(2, char::is_whitespace);
        let name = it.next().unwrap_or("").trim();
        if !name.is_empty() {
            let verdict = it.next().unwrap_or("end").trim();
            return Some(TestEvent::End(name.to_string(), verdict.to_string()));
        }
    }

    // 2) Rust libtest: "test some::name ... ok" (END) or a bare "test some::name ... " (START).
    //    Captured mode emits the whole line at once, so the same text yields END; --nocapture
    //    flushes the prefix before running the test, which is what makes START reachable at all.
    if let Some(rest) = raw.strip_prefix("test ") {
        // `" ... "` (with a verdict or not) and a bare `" ..."` at end of buffer are the two forms.
        let split = rest
            .split_once(" ... ")
            .or_else(|| rest.strip_suffix(" ...").map(|n| (n, "")));
        if let Some((name, after)) = split {
            let name = name.trim();
            if !name.is_empty() && !name.contains(' ') {
                let verdict = after.trim();
                return Some(if verdict.is_empty() {
                    TestEvent::Start(name.to_string())
                } else {
                    TestEvent::End(name.to_string(), first_token(verdict))
                });
            }
        }
    }

    // 3) pytest -v / -rA: "path/to/test_x.py::test_name PASSED" (also FAILED/ERROR/SKIPPED/XFAIL).
    if let Some((head, tail)) = s.rsplit_once(char::is_whitespace) {
        let verdict = first_token(tail);
        if head.contains("::")
            && !head.contains(' ')
            && matches!(
                verdict.as_str(),
                "PASSED" | "FAILED" | "ERROR" | "SKIPPED" | "XFAIL" | "XPASS"
            )
        {
            return Some(TestEvent::End(head.to_string(), verdict));
        }
    }
    // A pytest nodeid alone on the line (verbose + unbuffered) marks the START of that test.
    if s.contains("::") && !s.contains(' ') && s.len() > 2 {
        return Some(TestEvent::Start(s.to_string()));
    }

    // 4) TAP: "ok 12 - name" / "not ok 12 - name".
    for (prefix, verdict) in [("not ok ", "not ok"), ("ok ", "ok")] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Some((num, name)) = rest.split_once(" - ") {
                if num.trim().parse::<u64>().is_ok() && !name.trim().is_empty() {
                    return Some(TestEvent::End(name.trim().to_string(), verdict.to_string()));
                }
            }
        }
    }

    None
}

fn first_token(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

/// The last test seen to start, the last seen to finish, and how many finished.
#[derive(Debug, Default, Clone)]
pub struct TestTracker {
    /// The most recent test seen to BEGIN. On a hang this is the prime suspect.
    pub last_started: Option<String>,
    /// The most recent test seen to FINISH; it bounds the culprit even when none is named.
    pub last_completed: Option<String>,
    /// How many tests finished (the denominator for "how far into the suite did it get").
    pub completed: u64,
    /// How many tests began.
    pub started: u64,
}

impl TestTracker {
    fn observe(&mut self, ev: &TestEvent) {
        match ev {
            TestEvent::Start(name) => {
                self.last_started = Some(name.clone());
                self.started += 1;
            }
            TestEvent::End(name, _) => {
                // A captured-mode libtest line is both the start and the end of that test.
                if self.last_started.as_deref() != Some(name.as_str()) {
                    self.last_started = Some(name.clone());
                    self.started += 1;
                }
                self.last_completed = Some(name.clone());
                self.completed += 1;
            }
        }
    }
}

/// The runner's verdict about which test was responsible when a step had to be terminated.
#[derive(Debug, Clone)]
pub struct Culprit {
    /// The test's name, when one could be attributed.
    pub test: Option<String>,
    /// How the name was derived — quoted verbatim in reports so a reader can weigh it.
    pub how: &'static str,
    /// Tests that completed before the kill (the denominator for "how far did it get").
    pub completed: u64,
    /// The last test that DID complete, which bounds the culprit even when it is unnamed.
    pub last_completed: Option<String>,
}

impl Culprit {
    /// One-line human rendering for the step's outcome line.
    pub fn describe(&self) -> String {
        match &self.test {
            Some(name) => format!(
                "culprit test {name} ({}; {} test(s) completed first{})",
                self.how,
                self.completed,
                match &self.last_completed {
                    Some(l) => format!(", last completed {l}"),
                    None => String::new(),
                }
            ),
            None => format!(
                "no test-level attribution ({}; {} test(s) completed{})",
                self.how,
                self.completed,
                match &self.last_completed {
                    Some(l) => format!(", last completed {l}"),
                    None => String::new(),
                }
            ),
        }
    }
}

/// One step's live output state: the durable log, the unterminated tail, and the test tracker.
///
/// stdout and stderr share ONE of these so the tracker sees a single ordered view of the step and
/// a harness that writes progress to stderr is attributed just as well as one that uses stdout.
pub struct StepStream {
    tag: String,
    log: Mutex<Option<File>>,
    /// Bytes received since the last newline. On a hang this holds the hung test's start marker.
    partial: Mutex<String>,
    tracker: Mutex<TestTracker>,
    /// The last START already announced from an UNTERMINATED tail, so it is journaled once.
    tail_announced: Mutex<Option<String>>,
    evidence: Option<std::sync::Arc<RunEvidence>>,
}

impl StepStream {
    /// Open a step's stream, creating its durable log file when evidence capture is enabled.
    pub fn new(tag: &str, evidence: Option<std::sync::Arc<RunEvidence>>) -> Self {
        let log = evidence.as_ref().and_then(|e| e.open_step_log(tag));
        StepStream {
            tag: tag.to_string(),
            log: Mutex::new(log),
            partial: Mutex::new(String::new()),
            tracker: Mutex::new(TestTracker::default()),
            tail_announced: Mutex::new(None),
            evidence,
        }
    }

    /// Absorb a chunk of raw output: write it through to disk, then split and classify lines.
    pub fn ingest(&self, bytes: &[u8]) {
        if let Ok(mut guard) = self.log.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(bytes);
                let _ = f.flush();
            }
        }
        let text = String::from_utf8_lossy(bytes);
        let mut partial = match self.partial.lock() {
            Ok(p) => p,
            Err(_) => return,
        };
        partial.push_str(&text);
        // Split off every COMPLETE line, leaving the unterminated remainder in `partial`.
        while let Some(idx) = partial.find('\n') {
            let line: String = partial.drain(..=idx).collect();
            drop(partial);
            self.classify(line.trim_end_matches(['\n', '\r']));
            partial = match self.partial.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
        }
        // Bound the retained tail. A step that streams megabytes without a newline (a progress bar
        // rewriting with \r) must not grow this without limit; the marker we care about is short
        // and sits at the END, so keep the tail.
        if partial.len() > 8192 {
            let keep = partial.len() - 4096;
            let cut = (keep..partial.len())
                .find(|i| partial.is_char_boundary(*i))
                .unwrap_or(partial.len());
            let tail = partial[cut..].to_string();
            *partial = tail;
        }
        let tail = partial.clone();
        drop(partial);
        // ANNOUNCE AN UNTERMINATED START IMMEDIATELY, DO NOT WAIT FOR TEARDOWN. This is the
        // record that makes the journal survive a kill of the RUNNER ITSELF (a CI job
        // cancellation, an OOM, a SIGKILL from an outer supervisor) rather than only a kill of
        // the step: at that point nothing computes a verdict, and the newest `test_start` with no
        // matching `test_end` is the whole answer. It is journaled once per distinct marker.
        if let Some(TestEvent::Start(name)) = recognize(&tail) {
            let already = self
                .tail_announced
                .lock()
                .map(|g| g.as_deref() == Some(name.as_str()))
                .unwrap_or(true);
            if !already {
                if let Ok(mut g) = self.tail_announced.lock() {
                    *g = Some(name.clone());
                }
                if let Ok(mut t) = self.tracker.lock() {
                    t.observe(&TestEvent::Start(name.clone()));
                }
                if let Some(e) = &self.evidence {
                    e.record("test_start", &[("step", self.tag.clone()), ("test", name)]);
                }
            }
        }
    }

    fn classify(&self, line: &str) {
        let Some(ev) = recognize(line) else { return };
        // A line that COMPLETES a marker already announced from the tail clears the announcement,
        // so the same test is not counted as started twice.
        if let Ok(mut g) = self.tail_announced.lock() {
            let name = match &ev {
                TestEvent::Start(n) => n,
                TestEvent::End(n, _) => n,
            };
            if g.as_deref() == Some(name.as_str()) {
                *g = None;
                if let TestEvent::End(n, verdict) = &ev {
                    // Already counted as started; record only the completion.
                    if let Ok(mut t) = self.tracker.lock() {
                        t.last_completed = Some(n.clone());
                        t.completed += 1;
                    }
                    drop(g);
                    if let Some(e) = &self.evidence {
                        e.record(
                            "test_end",
                            &[
                                ("step", self.tag.clone()),
                                ("test", n.clone()),
                                ("verdict", verdict.clone()),
                            ],
                        );
                    }
                    return;
                }
                return; // a duplicate START announcement
            }
        }
        if let Ok(mut t) = self.tracker.lock() {
            t.observe(&ev);
        }
        // Emit the boundary to the journal IMMEDIATELY. This is the record that survives a kill:
        // the last `test_start` with no matching `test_end` names the culprit even if the runner
        // itself is SIGKILLed before it can compute a verdict.
        if let Some(e) = &self.evidence {
            match ev {
                TestEvent::Start(name) => {
                    e.record("test_start", &[("step", self.tag.clone()), ("test", name)])
                }
                TestEvent::End(name, verdict) => e.record(
                    "test_end",
                    &[
                        ("step", self.tag.clone()),
                        ("test", name),
                        ("verdict", verdict),
                    ],
                ),
            }
        }
    }

    /// Flush any unterminated tail into the tracker and return the attribution verdict.
    ///
    /// Called at teardown. The tail is classified here and NOT during `ingest`, because a partial
    /// line is only meaningful once we know no more bytes are coming — mid-stream it is simply an
    /// incomplete line that the next chunk will finish.
    pub fn culprit(&self) -> Culprit {
        // Trim only the LEADING side and hard line endings: the trailing space in
        // `test name ... ` is the signal, not noise.
        let tail = self
            .partial
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
            .trim_start()
            .trim_end_matches(['\n', '\r'])
            .to_string();
        let tail_event = if tail.trim().is_empty() {
            None
        } else {
            recognize(&tail)
        };
        let t = match self.tracker.lock() {
            Ok(t) => t.clone(),
            Err(_) => TestTracker::default(),
        };
        // 1) An unterminated START marker in the tail is the strongest evidence there is: the
        //    harness announced the test and never got to print its verdict.
        if let Some(TestEvent::Start(name)) = tail_event {
            return Culprit {
                test: Some(name),
                how: "announced in the unterminated output tail, never completed",
                completed: t.completed,
                last_completed: t.last_completed,
            };
        }
        // 2) Otherwise a test that started and never ended.
        if let Some(started) = t.last_started.clone() {
            if t.last_completed.as_deref() != Some(started.as_str()) {
                return Culprit {
                    test: Some(started),
                    how: "started and never completed",
                    completed: t.completed,
                    last_completed: t.last_completed,
                };
            }
        }
        // 3) Every started test completed: the hang is after the last one (harness teardown, a
        //    leaked background process, or a harness we cannot parse). Name the boundary, not a
        //    test, because guessing here would be a false accusation.
        Culprit {
            test: None,
            how: if t.completed == 0 {
                "no test boundaries were recognized in this step's output"
            } else {
                "every recognized test completed; the step hung after the last one"
            },
            completed: t.completed,
            last_completed: t.last_completed,
        }
    }

    /// Snapshot of the tracker, for callers that want the raw counts.
    pub fn counts(&self) -> TestTracker {
        self.tracker
            .lock()
            .map(|t| t.clone())
            .unwrap_or_else(|_| TestTracker::default())
    }
}

/// Columns describing attribution, for callers that want them in a map.
///
/// Deliberately NOT part of the per-step profile CSV: that schema is asserted byte-identical
/// across engines, so widening it here would break a cross-check that exists to catch exactly
/// that kind of one-sided drift.
pub fn culprit_columns(c: &Culprit) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("culprit_test".into(), c.test.clone().unwrap_or_default());
    m.insert("culprit_basis".into(), c.how.into());
    m.insert("tests_completed".into(), c.completed.to_string());
    m.insert(
        "last_completed_test".into(),
        c.last_completed.clone().unwrap_or_default(),
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libtest_completed_line_is_an_end() {
        assert_eq!(
            recognize("test detcore::sched::race ... ok"),
            Some(TestEvent::End("detcore::sched::race".into(), "ok".into()))
        );
        assert_eq!(
            recognize("test detcore::sched::race ... FAILED"),
            Some(TestEvent::End(
                "detcore::sched::race".into(),
                "FAILED".into()
            ))
        );
    }

    #[test]
    fn libtest_bare_prefix_is_a_start() {
        assert_eq!(
            recognize("test detcore::sched::hang ... "),
            Some(TestEvent::Start("detcore::sched::hang".into()))
        );
    }

    #[test]
    fn pytest_and_tap_and_explicit_protocol() {
        assert_eq!(
            recognize("tests/test_a.py::test_b PASSED"),
            Some(TestEvent::End(
                "tests/test_a.py::test_b".into(),
                "PASSED".into()
            ))
        );
        assert_eq!(
            recognize("ok 7 - the seventh thing"),
            Some(TestEvent::End("the seventh thing".into(), "ok".into()))
        );
        assert_eq!(
            recognize("not ok 8 - the eighth thing"),
            Some(TestEvent::End("the eighth thing".into(), "not ok".into()))
        );
        assert_eq!(
            recognize("##TEST-START my_case"),
            Some(TestEvent::Start("my_case".into()))
        );
        assert_eq!(
            recognize("##TEST-END my_case ok"),
            Some(TestEvent::End("my_case".into(), "ok".into()))
        );
    }

    #[test]
    fn ordinary_output_is_not_mistaken_for_a_test() {
        // A false positive renames the culprit, so these must all stay unrecognized.
        for line in [
            "",
            "Compiling hermit v0.1.0",
            "test suite finished",
            "ok",
            "ok then, moving on",
            "running 42 tests",
            "note: test failed, to rerun pass `--lib`",
        ] {
            assert_eq!(recognize(line), None, "misrecognized: {line:?}");
        }
    }

    #[test]
    fn culprit_prefers_the_unterminated_tail() {
        let s = StepStream::new("g:j", None);
        s.ingest(b"test a::one ... ok\ntest a::two ... ok\ntest a::hangs ... ");
        let c = s.culprit();
        assert_eq!(c.test.as_deref(), Some("a::hangs"));
        assert_eq!(c.completed, 2);
        assert_eq!(c.last_completed.as_deref(), Some("a::two"));
    }

    #[test]
    fn culprit_uses_started_without_end_when_the_tail_is_clean() {
        let s = StepStream::new("g:j", None);
        s.ingest(b"##TEST-START alpha\n##TEST-END alpha ok\n##TEST-START beta\nnoise\n");
        let c = s.culprit();
        assert_eq!(c.test.as_deref(), Some("beta"));
        assert_eq!(c.completed, 1);
    }

    #[test]
    fn a_clean_suite_accuses_nobody() {
        let s = StepStream::new("g:j", None);
        s.ingest(b"test a::one ... ok\ntest a::two ... ok\ntest result: ok. 2 passed\n");
        let c = s.culprit();
        assert_eq!(c.test, None);
        assert_eq!(c.completed, 2);
        assert_eq!(c.last_completed.as_deref(), Some("a::two"));
    }

    #[test]
    fn unparsable_output_degrades_to_naming_no_test() {
        let s = StepStream::new("g:j", None);
        s.ingest(b"building...\nlinking...\n");
        let c = s.culprit();
        assert_eq!(c.test, None);
        assert_eq!(c.completed, 0);
    }

    #[test]
    fn partial_tail_is_bounded() {
        let s = StepStream::new("g:j", None);
        let blob = vec![b'x'; 100_000];
        s.ingest(&blob);
        assert!(s.partial.lock().unwrap().len() <= 8192);
    }

    #[test]
    fn nonces_are_unique_per_step() {
        let a = mint_step_nonce("g:j");
        let b = mint_step_nonce("g:j");
        assert_ne!(a, b);
        assert!(a.ends_with("g_j"));
    }
}

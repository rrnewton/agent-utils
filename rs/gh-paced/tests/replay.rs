//! Incident replay: the publisher burst that preceded an account suspension, driven through the
//! pacing engine with a fake clock and a fake gh.
//!
//! The original burst (2026-10-04, 11:04:12 to 11:04:50 US Eastern, 38.5 s) was 44 gh calls:
//! one summary comment and 21 "part" comments, each posted with
//! `gh api --method POST <issue>/comments --input <request.json>` and immediately read back with
//! `gh api <comment>` (GET). The summary body was 4,018 bytes of prose; parts 1-20 were
//! 60,577-byte bodies holding one 60,000-character base64 line, and part 21 a 30,958-byte body
//! with a 30,380-character line. The call offsets below are the measured start times, in
//! seconds after the first call. Repository, issue and comment identifiers are placeholders and
//! the bodies are synthetic with the same shape and sizes.

use gh_paced::classify::Class;
use gh_paced::clock::{Clock, FakeClock};
use gh_paced::config::{ClassLimits, Config};
use gh_paced::pushback::Scanner;
use gh_paced::runner::{Captured, Exit, Invocation, Ran, Runner};
use gh_paced::state::{self, Holder, Paths};
use gh_paced::timefmt::human;
use gh_paced::wrapper::{Outcome, Wrapper, EXIT_CONTENT, EXIT_REFUSED};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Measured start offsets of the 44 calls, seconds after 11:04:12.356 ET. Even indices are the
/// POSTs (summary, then parts 1..=21); odd indices are the read-backs.
const OFFSETS: [f64; 44] = [
    0.0, 0.894, 1.372, 2.636, 3.279, 4.579, 5.157, 6.427, 6.966, 8.265, 8.861, 10.101, 10.685,
    12.016, 12.548, 13.652, 14.46, 15.56, 16.095, 17.454, 18.02, 19.68, 20.212, 21.32, 21.904,
    22.963, 23.479, 24.649, 25.147, 26.269, 26.854, 28.145, 28.723, 29.844, 30.392, 31.549, 32.07,
    33.395, 33.998, 35.247, 35.907, 36.968, 37.494, 38.485,
];

/// 2026-10-04T15:04:12.356Z, the first call.
const T0: f64 = 1_791_126_252.356;
const HOUR: f64 = 3600.0;

/// Simulated duration of one gh call (the originals took 0.5 to 1.3 s).
const CALL_SECS: f64 = 0.6;

const ENDPOINT: &str = "repos/o/r/issues/1/comments";

#[derive(Debug, Clone)]
struct Call {
    at: f64,
    args: Vec<String>,
}

/// A fake gh: records every call with the fake time, takes `CALL_SECS`, prints nothing on stderr
/// unless told to, and answers `api rate_limit` with a configurable snapshot.
struct FakeGh<'a> {
    clock: &'a FakeClock,
    runs: Mutex<Vec<Call>>,
    captures: Mutex<Vec<f64>>,
    /// When set, each refresh records the READ hourly window as saved on disk at that moment.
    state_dir: Mutex<Option<PathBuf>>,
    read_at_capture: Mutex<Vec<u64>>,
    core_remaining: Mutex<(u64, f64)>,
    stderr: Mutex<Vec<u8>>,
}

impl<'a> FakeGh<'a> {
    fn new(clock: &'a FakeClock) -> Self {
        Self {
            clock,
            runs: Mutex::new(Vec::new()),
            captures: Mutex::new(Vec::new()),
            state_dir: Mutex::new(None),
            read_at_capture: Mutex::new(Vec::new()),
            core_remaining: Mutex::new((4500, T0 + 3600.0)),
            stderr: Mutex::new(Vec::new()),
        }
    }

    fn runs(&self) -> Vec<Call> {
        self.runs.lock().unwrap().clone()
    }

    fn posts(&self) -> Vec<Call> {
        self.runs()
            .into_iter()
            .filter(|c| c.args.iter().any(|a| a == "POST"))
            .collect()
    }

    fn gets(&self) -> Vec<Call> {
        self.runs()
            .into_iter()
            .filter(|c| !c.args.iter().any(|a| a == "POST"))
            .collect()
    }
}

impl Runner for FakeGh<'_> {
    fn run(&self, inv: Invocation<'_>, scanner: &mut Scanner) -> Result<Ran, String> {
        self.runs.lock().unwrap().push(Call {
            at: self.clock.now(),
            args: inv.args.to_vec(),
        });
        let err = self.stderr.lock().unwrap().clone();
        scanner.feed(&err);
        self.clock.advance_to(self.clock.now() + CALL_SECS);
        Ok(Ran {
            exit: Exit::Code(0),
            deadline_hit: false,
            late_signal: None,
            cut_off: false,
        })
    }

    fn capture(&self, inv: Invocation<'_>, _timeout: f64) -> Result<Captured, String> {
        assert_eq!(inv.args, ["api", "rate_limit"]);
        self.captures.lock().unwrap().push(self.clock.now());
        if let Some(dir) = self.state_dir.lock().unwrap().as_deref() {
            self.read_at_capture
                .lock()
                .unwrap()
                .push(window_cost(dir, Class::Read));
        }
        let (remaining, reset) = *self.core_remaining.lock().unwrap();
        let body = format!(
            r#"{{"resources":{{"core":{{"limit":5000,"used":{},"remaining":{remaining},"reset":{}}},
            "graphql":{{"limit":5000,"used":0,"remaining":5000,"reset":{}}},
            "search":{{"limit":30,"used":0,"remaining":30,"reset":{}}}}}}}"#,
            5000 - remaining,
            reset as u64,
            reset as u64,
            (self.clock.now() + 60.0) as u64
        );
        self.clock.advance_to(self.clock.now() + 0.3);
        Ok(Captured {
            exit: Exit::Code(0),
            stdout: body.into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            stdout_cut: false,
            stderr_cut: false,
        })
    }
}

static NONCE: AtomicU64 = AtomicU64::new(1);

fn always_alive(_: &Holder) -> bool {
    true
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gh-paced-replay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run one gh command line as its own gh-paced process would.
fn gh(
    clock: &FakeClock,
    fake: &FakeGh<'_>,
    dir: &Path,
    cfg: &Config,
    args: &[String],
) -> (Outcome, Vec<String>) {
    let (outcome, messages, _) = gh_on(clock, fake, dir, cfg, args);
    (outcome, messages)
}

/// [`gh`] with any clock, also returning [`Wrapper::credential_quit`].
fn gh_on(
    clock: &dyn Clock,
    fake: &FakeGh<'_>,
    dir: &Path,
    cfg: &Config,
    args: &[String],
) -> (Outcome, Vec<String>, bool) {
    let alive = always_alive;
    let mut w = Wrapper {
        clock,
        runner: fake,
        cfg: cfg.clone(),
        paths: Paths::new(dir.to_path_buf(), "replay"),
        host: "testhost".into(),
        real_gh: PathBuf::from("/fake/gh"),
        echo: false,
        messages: Vec::new(),
        printed: 0,
        stdin_is_tty: true,
        stdin_reader: Box::new(|_| Ok(Vec::new())),
        chain: Vec::new(),
        depth: 0,
        alive: &alive,
        inherited_leases: Vec::new(),
        pid: 4242,
        start_ticks: 1,
        nonce: format!("{:016x}", NONCE.fetch_add(1, Ordering::SeqCst)),
        gh_aliases: gh_paced::alias::GhAliases::default(),
        alias_note: None,
        body_bytes_used: 0,
        editor_guard_env: Vec::new(),
        self_exe: None,
        stderr_deadline: None,
        credential_quit: false,
    };
    let outcome = w.run(args);
    (outcome, w.messages, w.credential_quit)
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// Deterministic base64 text of `n` characters (mixed case and digits, like real base64).
fn base64ish(n: usize, seed: u64) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ALPHABET[(x >> 58) as usize] as char
        })
        .collect()
}

/// Plain prose of exactly `n` bytes.
fn prose(n: usize) -> String {
    let sentence =
        "The validation run finished; the logs stay on the host, see the path and sha256 above. ";
    let mut s = String::new();
    while s.len() < n {
        s.push_str(sentence);
    }
    s.truncate(n);
    s
}

fn write_request(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(format!("{name}.request.json"));
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({ "body": body })).unwrap(),
    )
    .unwrap();
    path.display().to_string()
}

/// The incident's 22 request files: a 4,018-byte prose summary, then parts with one long base64
/// line each (60,000 characters for parts 1-20, 30,380 for part 21).
fn incident_requests(dir: &Path) -> Vec<String> {
    let mut out = vec![write_request(dir, "SUMMARY", &prose(4018))];
    for part in 1..=21u64 {
        let run = if part == 21 { 30_380 } else { 60_000 };
        let header = format!("artifact bundle part {part:02}/21 (tar.xz, base64)\n\n```base64\n");
        let body = format!("{header}{}\n```", base64ish(run, part));
        out.push(write_request(dir, &format!("part-{part:02}"), &body));
    }
    out
}

fn post_args(request: &str) -> Vec<String> {
    strings(&["api", "--method", "POST", ENDPOINT, "--input", request])
}

fn get_args(i: usize) -> Vec<String> {
    vec![
        "api".into(),
        format!("repos/o/r/issues/comments/{}", 1000 + i),
    ]
}

/// Drive the 44-call sequence. Each call starts at its original offset, or as soon as the previous
/// call returned when pacing has pushed the script past that offset.
fn replay(
    clock: &FakeClock,
    fake: &FakeGh<'_>,
    dir: &Path,
    cfg: &Config,
    requests: &[String],
) -> Vec<(Outcome, Vec<String>)> {
    let mut results = Vec::new();
    for (i, offset) in OFFSETS.iter().enumerate() {
        clock.advance_to(T0 + offset);
        let args = if i % 2 == 0 {
            post_args(&requests[i / 2])
        } else {
            get_args(i / 2)
        };
        results.push(gh(clock, fake, dir, cfg, &args));
    }
    results
}

fn window_cost(dir: &Path, class: Class) -> u64 {
    let paths = Paths::new(dir.to_path_buf(), "replay");
    let st = state::load_readonly(&paths).unwrap();
    st.buckets
        .get(class.name())
        .map(|b| b.window.iter().map(|(_, c)| u64::from(*c)).sum())
        .unwrap_or(0)
}

#[test]
fn incident_parts_are_refused_and_the_summary_is_allowed() {
    let dir = scratch("parts");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    let requests = incident_requests(&dir);
    let results = replay(&clock, &fake, &dir, &cfg, &requests);

    assert_eq!(
        results[0].0,
        Outcome::Exit(0),
        "summary post: {:?}",
        results[0].1
    );
    for part in 1..=21 {
        let (outcome, messages) = &results[part * 2];
        assert_eq!(
            *outcome,
            Outcome::Exit(EXIT_CONTENT),
            "part {part}: {messages:?}"
        );
        let text = messages.join("\n");
        assert!(
            text.contains("GH-PACED REFUSED [replay] content guard"),
            "{text}"
        );
        assert!(text.contains("8192"), "size limit named: {text}");
        assert!(text.contains("base64"), "base64 run named: {text}");
    }
    let posts = fake.posts();
    assert_eq!(posts.len(), 1, "only the summary reached gh");
    assert!(posts[0]
        .args
        .iter()
        .any(|a| a.ends_with("SUMMARY.request.json")));
    // Every read-back still ran (as READ), and gh-paced added one rate_limit refresh.
    assert_eq!(fake.gets().len(), 22);
    assert_eq!(fake.captures.lock().unwrap().len(), 1);
    assert_eq!(window_cost(&dir, Class::Write), 1);
    assert_eq!(window_cost(&dir, Class::Read), 23, "22 GETs + 1 refresh");
    eprintln!(
        "replay parts: part 01 refusal: {}",
        results[2].1.join(" | ")
    );
    eprintln!(
        "replay parts: last call returned at +{:.1} s; gh ran {} times (1 POST, {} GETs) plus {} rate_limit refresh",
        clock.now() - T0,
        fake.runs().len(),
        fake.gets().len(),
        fake.captures.lock().unwrap().len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prose_bodies_are_spaced_30_seconds_apart_and_gets_use_the_read_bucket() {
    let dir = scratch("prose");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    let requests: Vec<String> = (0..22)
        .map(|i| write_request(&dir, &format!("prose-{i:02}"), &prose(4096)))
        .collect();
    let results = replay(&clock, &fake, &dir, &cfg, &requests);

    for (i, (outcome, messages)) in results.iter().enumerate() {
        assert_eq!(*outcome, Outcome::Exit(0), "call {i}: {messages:?}");
    }
    let posts = fake.posts();
    assert_eq!(posts.len(), 22);
    for pair in posts.windows(2) {
        let gap = pair[1].at - pair[0].at;
        assert!(gap >= 30.0 - 1e-6, "writes {gap} s apart");
    }
    let span = posts[21].at - posts[0].at;
    assert!(
        span >= 21.0 * 30.0 - 1e-6,
        "22 posts took {span} s, expected >= 630 s"
    );
    // The original burst took 37.5 s from first to last post.
    assert!(span > 600.0);
    // Throttled posts announced the wait.
    let all: Vec<String> = results.iter().flat_map(|(_, m)| m.clone()).collect();
    let warnings = all
        .iter()
        .filter(|m| m.starts_with("GH-PACED WARNING [replay] write budget 1 per 30 s"))
        .count();
    assert!(
        warnings >= 21,
        "one warning per throttled post, got {warnings}: {all:?}"
    );
    assert!(all
        .iter()
        .any(|m| m.contains("sleeping") && m.contains("ET)")));
    // Reads are charged to the read bucket, writes to the write bucket.
    let refreshes = fake.captures.lock().unwrap().len() as u64;
    assert!(
        refreshes >= 2,
        "snapshot refreshed every 300 s over ~11 min"
    );
    assert_eq!(window_cost(&dir, Class::Write), 22);
    assert_eq!(window_cost(&dir, Class::Read), 22 + refreshes);
    assert_eq!(fake.gets().len(), 22);
    let gaps: Vec<f64> = posts.windows(2).map(|p| p[1].at - p[0].at).collect();
    eprintln!(
        "replay prose: 22 posts from +{:.1} s to +{:.1} s (span {span:.1} s, min gap {:.3} s); \
         {refreshes} rate_limit refreshes; total slept {:.1} s; first warning: {}",
        posts[0].at - T0,
        posts[21].at - T0,
        gaps.iter().cloned().fold(f64::INFINITY, f64::min),
        clock.total_slept(),
        all.iter()
            .find(|m| m.starts_with("GH-PACED WARNING"))
            .unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hourly_write_cap_holds() {
    let dir = scratch("hourly");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    let request = write_request(&dir, "prose", &prose(4096));
    for i in 0..30 {
        let (outcome, m) = gh(&clock, &fake, &dir, &cfg, &post_args(&request));
        assert_eq!(outcome, Outcome::Exit(0), "post {i}: {m:?}");
    }
    let first = fake.posts()[0].at;
    // The 31st needs the first to leave the hour window: ~2,700 s away, past the 900 s bound.
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &post_args(&request));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    let text = messages.join("\n");
    assert!(
        text.contains("write budget 30/hour per host reached (30 used in the last hour)"),
        "{text}"
    );
    assert!(text.contains("GH_PACED_MAX_WAIT=900"), "{text}");
    assert_eq!(fake.posts().len(), 30);
    // With a longer allowed wait it sleeps instead, and runs only once the hour has passed.
    let patient = Config {
        max_wait_secs: 4000.0,
        ..Config::default()
    };
    let (outcome, messages) = gh(&clock, &fake, &dir, &patient, &post_args(&request));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    let posts = fake.posts();
    assert_eq!(posts.len(), 31);
    assert!(
        posts[30].at >= first + 3600.0,
        "31st post at +{} s",
        posts[30].at - first
    );
    let in_hour = posts
        .iter()
        .filter(|p| p.at > posts[30].at - 3600.0)
        .count();
    assert!(in_hour <= 30, "{in_hour} posts in the last hour");
    eprintln!(
        "replay hourly: post 31 refused under the 900 s bound ({}); admitted at +{:.1} s with a 4000 s bound",
        text.lines().find(|l| l.contains("30/hour")).unwrap_or(""),
        posts[30].at - first
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn low_account_budget_blocks_every_api_call_loudly() {
    let dir = scratch("block");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    *fake.core_remaining.lock().unwrap() = (900, T0 + 1800.0); // 18% left, resets in 30 min
    let cfg = Config::default();
    let request = write_request(&dir, "prose", &prose(1000));
    let calls = [
        post_args(&request),
        get_args(1),
        strings(&["pr", "view", "5"]),
        strings(&["search", "issues", "flaky"]),
        strings(&["api", "graphql", "-f", "query=query { viewer { login } }"]),
    ];
    for args in &calls {
        let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, args);
        assert_eq!(
            outcome,
            Outcome::Exit(EXIT_REFUSED),
            "{args:?}: {messages:?}"
        );
        let text = messages.join("\n");
        assert!(text.contains("at or below the 20% floor"), "{text}");
        assert!(text.contains("GH-PACED REFUSED [replay]"), "{text}");
    }
    assert!(
        fake.runs().is_empty(),
        "nothing reached gh: {:?}",
        fake.runs()
    );
    let (_, sample) = gh(&clock, &fake, &dir, &cfg, &get_args(9));
    eprintln!("replay block: {}", sample.join(" | "));
    // A caller allowed to wait past the reset sleeps until then and runs.
    let patient = Config {
        max_wait_secs: 3600.0,
        ..Config::default()
    };
    let (outcome, messages) = gh(&clock, &fake, &dir, &patient, &get_args(2));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert!(messages
        .iter()
        .any(|m| m.contains("blocking until it resets") && m.contains("sleeping")));
    // GitHub reports `reset` in whole Unix seconds; the fake truncates the same way.
    let reset = ((T0 + 1800.0) as u64) as f64;
    assert!(
        fake.runs()[0].at >= reset,
        "ran {} s before the reset",
        reset - fake.runs()[0].at
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn half_account_budget_halves_local_rates() {
    let dir = scratch("halve");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    *fake.core_remaining.lock().unwrap() = (2000, T0 + 3000.0); // 40% left
    let cfg = Config::default();
    let mut outcomes = Vec::new();
    for i in 0..6 {
        outcomes.push(gh(&clock, &fake, &dir, &cfg, &get_args(i)));
    }
    let text: Vec<String> = outcomes.iter().flat_map(|(_, m)| m.clone()).collect();
    assert!(
        text.iter()
            .any(|m| m.contains("rates halved to 10/min and 250/hour")),
        "{text:?}"
    );
    // Halved READ: burst 5 (refresh charged 1), so the 5th GET waits for a token at 10/min.
    let sleeps = clock.sleeps();
    assert!(!sleeps.is_empty(), "halved burst forced a wait");
    assert!(
        sleeps.iter().all(|s| *s <= 6.0 + 1e-6),
        "one token per 6 s: {sleeps:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pushback_sets_a_cooldown_that_blocks_the_next_calls() {
    let dir = scratch("pushback");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    *fake.stderr.lock().unwrap() =
        b"gh: You have exceeded a secondary rate limit. Please wait a few minutes. (HTTP 403)\n"
            .to_vec();
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(0));
    assert_eq!(outcome, Outcome::Exit(0), "gh's own status passes through");
    let text = messages.join("\n");
    assert!(text.contains("GH-PACED PUSHBACK [replay]"), "{text}");
    assert!(text.contains("secondary rate limit"), "{text}");
    assert!(text.contains("paused for 900 s"), "{text}");
    fake.stderr.lock().unwrap().clear();
    // Every class that reaches GitHub now waits out the 15-minute cooldown.
    let short = Config {
        max_wait_secs: 60.0,
        ..Config::default()
    };
    for args in [
        get_args(1),
        strings(&["pr", "comment", "1", "--body", "hi"]),
        strings(&["auth", "git-credential", "get"]),
    ] {
        let (outcome, messages) = gh(&clock, &fake, &dir, &short, &args);
        assert_eq!(
            outcome,
            Outcome::Exit(EXIT_REFUSED),
            "{args:?}: {messages:?}"
        );
        assert!(messages
            .join("\n")
            .contains("cooldown after GitHub pushback"));
    }
    assert_eq!(fake.runs().len(), 1);
    // LOCAL commands are never paced.
    let (outcome, _) = gh(&clock, &fake, &dir, &short, &strings(&["--version"]));
    assert_eq!(outcome, Outcome::Exit(0));
    // After the cooldown the next call runs.
    clock.advance_to(T0 + 901.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &short, &get_args(2));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retry_after_longer_than_the_default_is_honoured() {
    let dir = scratch("retry-after");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    *fake.stderr.lock().unwrap() = b"HTTP 429: Too Many Requests\nRetry-After: 3600\n".to_vec();
    let _ = gh(&clock, &fake, &dir, &cfg, &get_args(0));
    fake.stderr.lock().unwrap().clear();
    clock.advance_to(T0 + 1000.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(1));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every budget decision uses a time read after the state lock is held. A call that waited for
/// the lock while another process held it must not act on the time it started waiting: its
/// window entries would be back-dated and its waits computed from a stale clock.
#[test]
fn time_is_read_after_the_lock_is_taken() {
    let dir = scratch("late-lock");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    let paths = Paths::new(dir.clone(), "replay");
    let held = state::lock(&paths).unwrap();
    std::thread::scope(|s| {
        let worker = s.spawn(|| {
            gh(
                &clock,
                &fake,
                &dir,
                &cfg,
                &strings(&["pr", "comment", "1", "--body", "hi"]),
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!worker.is_finished(), "the call waits for the lock");
        assert!(fake.runs().is_empty());
        clock.advance_to(T0 + 100.0);
        drop(held);
        let (outcome, messages) = worker.join().unwrap();
        assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    });
    let st = state::load_readonly(&paths).unwrap();
    let write = &st.buckets["write"].window;
    assert_eq!(write.len(), 1, "{write:?}");
    for (class, bucket) in &st.buckets {
        assert!(
            bucket.window.iter().all(|(t, _)| *t >= T0 + 100.0),
            "{class} charged at the pre-lock time: {:?}",
            bucket.window
        );
    }
    assert!(fake.runs()[0].at >= T0 + 100.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupt state file is moved aside and replaced by a conservative state: every class is
/// blocked for the next hour and a pause runs, so a host that lost its history cannot burst. The
/// conservative state is saved (the next call does not start fresh), and it ends after an hour.
#[test]
fn corrupt_state_pauses_the_account_for_an_hour() {
    let dir = scratch("corrupt");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config {
        max_wait_secs: 60.0,
        ..Config::default()
    };
    let garbage = b"{\"buckets\": {\"write\": {\"level\": 1.0, \"wind";
    std::fs::write(dir.join("replay.json"), garbage).unwrap();
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(0));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    let text = messages.join("\n");
    assert!(text.contains("is unusable"), "{text}");
    assert!(
        text.contains("treated as used up for the next hour"),
        "{text}"
    );
    // The refusal names the longest wait: the hour-long recovery block, not the shorter pause.
    let block_ends = human(T0 + HOUR, T0, cfg.display_tz);
    assert!(
        text.contains(&format!(
            "read budget blocked until {block_ends} after state recovery"
        )),
        "{text}"
    );
    let aside = dir.join(format!("replay.json.corrupt-{}", T0.floor() as i64));
    assert_eq!(
        std::fs::read(&aside).unwrap(),
        garbage,
        "kept for inspection"
    );
    let paths = Paths::new(dir.clone(), "replay");
    let st = state::load_readonly(&paths).unwrap();
    let cd = st.cooldown.expect("a recovery pause");
    assert_eq!(cd.command, gh_paced::wrapper::RECOVERY_COMMAND);
    assert!((cd.until - (T0 + cfg.cooldown_secs)).abs() < 1.0, "{cd:?}");
    for class in [
        Class::Read,
        Class::Write,
        Class::Search,
        Class::GitCredential,
    ] {
        let b = &st.buckets[class.name()];
        let until = b.blocked_until.expect("a recovery block on disk");
        assert!((until - (T0 + HOUR)).abs() < 1e-6, "{class:?} {b:?}");
        assert_eq!(b.level, 0.0, "{class:?} starts with no tokens");
    }
    // The recovery state was saved: the next call is refused without a second recovery.
    let (outcome, messages) = gh(
        &clock,
        &fake,
        &dir,
        &cfg,
        &strings(&["pr", "comment", "1", "--body", "hi"]),
    );
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    let text = messages.join("\n");
    assert!(!text.contains("is unusable"), "{text}");
    assert!(
        text.contains(&format!(
            "write budget blocked until {block_ends} after state recovery"
        )),
        "{text}"
    );
    // After the pause, the block still holds.
    clock.advance_to(T0 + 901.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(1));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(
        messages.join("\n").contains(&format!(
            "read budget blocked until {block_ends} after state recovery"
        )),
        "{messages:?}"
    );
    assert!(fake.runs().is_empty() && fake.captures.lock().unwrap().is_empty());
    // An hour later calls run again.
    clock.advance_to(T0 + 3601.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(2));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert_eq!(fake.runs().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The recovery block does not depend on the limits in force when it was written. A host that
/// recovers under a tight write limit and is then given a far larger one stays blocked until the
/// hour is up: the lost history may have held a full hour of calls under either limit.
#[test]
fn recovery_block_survives_raised_limits() {
    let dir = scratch("raised");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let tight = Config {
        max_wait_secs: 60.0,
        write: ClassLimits {
            per_hour: 1,
            ..Config::default().write
        },
        ..Config::default()
    };
    std::fs::write(dir.join("replay.json"), b"not json").unwrap();
    let comment = strings(&["pr", "comment", "1", "--body", "hi"]);
    let (outcome, messages) = gh(&clock, &fake, &dir, &tight, &comment);
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(messages.join("\n").contains("is unusable"), "{messages:?}");
    let raised = Config {
        max_wait_secs: 60.0,
        write: ClassLimits {
            per_hour: 5000,
            ..Config::default().write
        },
        read: ClassLimits {
            per_hour: 5000,
            ..Config::default().read
        },
        ..Config::default()
    };
    let block_ends = human(T0 + HOUR, T0 + 901.0, raised.display_tz);
    for at in [901.0, 1800.0, 3500.0] {
        clock.advance_to(T0 + at);
        let (outcome, messages) = gh(&clock, &fake, &dir, &raised, &comment);
        assert_eq!(
            outcome,
            Outcome::Exit(EXIT_REFUSED),
            "at +{at}: {messages:?}"
        );
        let text = messages.join("\n");
        assert!(
            text.contains("write budget blocked until") && text.contains("after state recovery"),
            "at +{at}: {text}"
        );
        if at == 901.0 {
            assert!(text.contains(&block_ends), "{text}");
        }
    }
    assert!(fake.runs().is_empty() && fake.captures.lock().unwrap().is_empty());
    clock.advance_to(T0 + HOUR + 1.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &raised, &comment);
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert_eq!(fake.runs().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The account-wide snapshot is re-read after a sleep: a caller that slept out a cooldown
/// refreshes before it runs and obeys what the new snapshot says.
#[test]
fn feedback_is_rechecked_after_a_sleep() {
    let dir = scratch("recheck");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    *fake.stderr.lock().unwrap() =
        b"gh: You have exceeded a secondary rate limit. (HTTP 403)\n".to_vec();
    let _ = gh(&clock, &fake, &dir, &cfg, &get_args(0));
    fake.stderr.lock().unwrap().clear();
    // While the caller below sleeps, the account drops to 18%, under the 20% floor.
    *fake.core_remaining.lock().unwrap() = (900, T0 + 3600.0);
    clock.advance_to(T0 + 10.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(1));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(
        clock.sleeps().iter().any(|s| *s > 850.0),
        "slept out the cooldown: {:?}",
        clock.sleeps()
    );
    let captures = fake.captures.lock().unwrap().clone();
    assert_eq!(captures.len(), 2, "{captures:?}");
    assert!(
        captures[1] >= T0 + 900.0,
        "refreshed after the sleep: {captures:?}"
    );
    assert!(
        messages.join("\n").contains("at or below the 20% floor"),
        "{messages:?}"
    );
    assert_eq!(fake.runs().len(), 1, "the second call never ran");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A rate-limit refresh is itself a READ request: it is charged before it is sent, and when the
/// READ budget has no room the call waits for it rather than sending an unpaid refresh, so
/// refreshes cannot push READ past its cap.
#[test]
fn refresh_requests_are_charged_before_they_are_sent() {
    let dir = scratch("refresh-charge");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config {
        read: ClassLimits {
            per_minute: 60.0,
            burst: 10.0,
            per_hour: 4,
        },
        max_wait_secs: 60.0,
        ..Config::default()
    };
    *fake.state_dir.lock().unwrap() = Some(dir.clone());
    // The first call's refresh and three calls fill the hourly cap of 4.
    for i in 0..3 {
        let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(i));
        assert_eq!(outcome, Outcome::Exit(0), "{i}: {messages:?}");
    }
    assert_eq!(window_cost(&dir, Class::Read), 4);
    // The refresh's token was already saved in the state file when the request was sent.
    assert_eq!(
        *fake.read_at_capture.lock().unwrap(),
        [1],
        "charged before it was sent"
    );
    // Later the snapshot is due for a refresh by age, but there is no READ token for it.
    clock.advance_to(T0 + 400.0);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(3));
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert_eq!(fake.captures.lock().unwrap().len(), 1, "no unpaid refresh");
    assert_eq!(
        window_cost(&dir, Class::Read),
        4,
        "READ stayed within its cap"
    );
    assert_eq!(fake.runs().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// After a pushback the snapshot is discarded. If READ has no token left for the refresh when
/// the pause ends, WRITE and SEARCH calls wait for one (or are refused past
/// `GH_PACED_MAX_WAIT`) instead of running without current account feedback.
#[test]
fn exhausted_read_budget_does_not_skip_overdue_feedback() {
    let dir = scratch("refresh-wait");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let read = ClassLimits {
        per_minute: 60.0,
        burst: 10.0,
        per_hour: 4,
    };
    let cfg = Config {
        read,
        max_wait_secs: 60.0,
        ..Config::default()
    };
    // The first call's refresh and three calls fill the hourly READ cap of 4.
    for i in 0..3 {
        let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &get_args(i));
        assert_eq!(outcome, Outcome::Exit(0), "{i}: {messages:?}");
    }
    assert_eq!(window_cost(&dir, Class::Read), 4);
    // A write is pushed back: a 15-minute pause, and the snapshot is discarded.
    *fake.stderr.lock().unwrap() =
        b"gh: You have exceeded a secondary rate limit. (HTTP 403)\n".to_vec();
    let comment = strings(&["pr", "comment", "1", "--body", "hi"]);
    let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &comment);
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    fake.stderr.lock().unwrap().clear();
    let paths = Paths::new(dir.clone(), "replay");
    let st = state::load_readonly(&paths).unwrap();
    assert!(st.rate_limit.is_none(), "the snapshot was discarded");
    let pause_ends = st.cooldown.as_ref().expect("a pause").until;
    assert_eq!(fake.runs().len(), 4);
    assert_eq!(fake.captures.lock().unwrap().len(), 1);
    // The pause is over, but READ still has no token for the refresh.
    clock.advance_to(pause_ends + 1.0);
    let search = strings(&["search", "issues", "flaky"]);
    for args in [&comment, &search] {
        let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, args);
        assert_eq!(
            outcome,
            Outcome::Exit(EXIT_REFUSED),
            "{args:?}: {messages:?}"
        );
        let text = messages.join("\n");
        assert!(
            text.contains(
                "the account-wide rate-limit snapshot is due and the read budget has no token"
            ),
            "{args:?}: {text}"
        );
    }
    assert_eq!(fake.runs().len(), 4, "nothing ran without feedback");
    assert_eq!(fake.captures.lock().unwrap().len(), 1, "no unpaid refresh");
    assert_eq!(
        window_cost(&dir, Class::Read),
        4,
        "READ stayed within its cap"
    );
    // With room to wait, the write sleeps until READ has a token, refreshes, then runs.
    let patient = Config {
        max_wait_secs: 7200.0,
        ..cfg.clone()
    };
    let (outcome, messages) = gh(&clock, &fake, &dir, &patient, &comment);
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    let captures = fake.captures.lock().unwrap().clone();
    assert_eq!(captures.len(), 2, "{messages:?}");
    assert!(
        captures[1] >= T0 + HOUR,
        "the refresh waited for READ room: {captures:?}"
    );
    let runs = fake.runs();
    assert_eq!(runs.len(), 5);
    assert!(runs[4].at > captures[1], "the write ran after the refresh");
    assert!(
        window_cost(&dir, Class::Read) <= 4,
        "READ stayed within its cap"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A WRITE that finds another write in flight re-checks every 5 s and prints a warning on every
/// check, so a long wait is never silent; it runs once the other write has ended.
#[test]
fn in_flight_wait_warns_on_every_poll() {
    let dir = scratch("inflight-poll");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    let paths = Paths::new(dir.clone(), "replay");
    let other = "00000000000000ff";
    {
        let _g = state::lock(&paths).unwrap();
        let mut st = state::State::new();
        st.in_flight.push(Holder {
            pid: 7,
            start_ticks: 1,
            nonce: other.into(),
            class: Class::Write,
            since: T0,
            lease: None,
        });
        state::save(&paths, &st).unwrap();
    }
    let release = T0 + 20.0;
    let alive = |h: &Holder| h.nonce != other || clock.now() < release;
    let mut w = Wrapper {
        clock: &clock,
        runner: &fake,
        cfg: cfg.clone(),
        paths: paths.clone(),
        host: "testhost".into(),
        real_gh: PathBuf::from("/fake/gh"),
        echo: false,
        messages: Vec::new(),
        printed: 0,
        stdin_is_tty: true,
        stdin_reader: Box::new(|_| Ok(Vec::new())),
        chain: Vec::new(),
        depth: 0,
        alive: &alive,
        inherited_leases: Vec::new(),
        pid: 4242,
        start_ticks: 1,
        nonce: format!("{:016x}", NONCE.fetch_add(1, Ordering::SeqCst)),
        gh_aliases: gh_paced::alias::GhAliases::default(),
        alias_note: None,
        body_bytes_used: 0,
        editor_guard_env: Vec::new(),
        self_exe: None,
        stderr_deadline: None,
        credential_quit: false,
    };
    let outcome = w.run(&strings(&["issue", "comment", "1", "--body", "short note"]));
    assert_eq!(outcome, Outcome::Exit(0), "{:?}", w.messages);
    let warnings: Vec<&String> = w
        .messages
        .iter()
        .filter(|m| m.contains("1 write(s) already in flight on this host (pid 7)"))
        .collect();
    assert_eq!(
        warnings.len(),
        4,
        "one warning per 5 s check: {:?}",
        w.messages
    );
    assert!(
        warnings.iter().all(|m| m.contains("re-checking every 5 s")),
        "{warnings:?}"
    );
    let polls: Vec<f64> = clock.sleeps().into_iter().filter(|s| *s == 5.0).collect();
    assert_eq!(polls.len(), 4, "{:?}", clock.sleeps());
    let runs = fake.runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        runs[0].at >= release,
        "ran while the other write was in flight: {runs:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn credential_get() -> Vec<String> {
    strings(&["auth", "git-credential", "get"])
}

/// Start a pushback cooldown with one READ whose gh prints `stderr`, and return its end.
fn start_cooldown(clock: &FakeClock, fake: &FakeGh<'_>, dir: &Path, stderr: &[u8]) -> f64 {
    *fake.stderr.lock().unwrap() = stderr.to_vec();
    let (outcome, messages) = gh(clock, fake, dir, &Config::default(), &get_args(0));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    fake.stderr.lock().unwrap().clear();
    let st = state::load_readonly(&Paths::new(dir.to_path_buf(), "replay")).unwrap();
    st.cooldown.expect("a cooldown").until
}

const SECONDARY_403: &[u8] =
    b"gh: You have exceeded a secondary rate limit. Please wait a few minutes. (HTTP 403)\n";

/// git runs its credential helper inside a fetch or push that may hold the caller's locks, so
/// during a cooldown `auth git-credential get` is refused at once with one line, never slept
/// out (with the default 900 s GH_PACED_MAX_WAIT it used to sleep the whole cooldown).
#[test]
fn credential_get_is_refused_at_once_during_a_cooldown() {
    let dir = scratch("cred-cooldown");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let until = start_cooldown(&clock, &fake, &dir, SECONDARY_403);
    assert!(until - clock.now() > 850.0, "a 900 s cooldown");
    let cfg = Config::default();
    let runs = fake.runs().len();
    let captures = fake.captures.lock().unwrap().len();
    for (left, shown) in [(100.4, "101 s left"), (0.5, "1 s left")] {
        assert!(
            left < cfg.max_wait_secs,
            "GH_PACED_MAX_WAIT alone would sleep the rest out"
        );
        clock.advance_to(until - left);
        let before = clock.now();
        let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
        assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
        assert!(clock.sleeps().is_empty(), "slept: {:?}", clock.sleeps());
        assert_eq!(clock.now(), before, "no time passed");
        assert_eq!(fake.runs().len(), runs, "gh was not run");
        assert_eq!(
            fake.captures.lock().unwrap().len(),
            captures,
            "no GitHub request"
        );
        assert!(quit, "git is told to quit");
        assert_eq!(messages.len(), 1, "exactly one line: {messages:?}");
        let line = &messages[0];
        assert!(
            line.starts_with("GH-PACED REFUSED [replay] cooldown after GitHub pushback ("),
            "{line}"
        );
        assert!(
            line.contains("secondary rate limit"),
            "names the cause: {line}"
        );
        assert!(line.contains(shown), "names the seconds left: {line}");
        let ends = human(until, before, cfg.display_tz);
        assert!(line.contains(&format!("(ends {ends})")), "{line}");
        assert!(line.contains("(exit 75)"), "{line}");
    }
    // The refusals took no token: once the cooldown is over the next call runs at once.
    clock.advance_to(until + 1.0);
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert!(!quit);
    assert!(clock.sleeps().is_empty(), "slept: {:?}", clock.sleeps());
    assert_eq!(fake.runs().len(), runs + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The credential change shortens nothing else: a READ still sleeps out a cooldown and a WRITE
/// is still refused against GH_PACED_MAX_WAIT, even with the GIT_CREDENTIAL bound at zero.
#[test]
fn reads_and_writes_still_wait_out_a_cooldown() {
    let dir = scratch("cooldown-read-write");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let until = start_cooldown(&clock, &fake, &dir, SECONDARY_403);
    clock.advance_to(until - 100.4);
    let runs = fake.runs().len();
    let short = Config {
        max_wait_secs: 60.0,
        git_credential_max_wait_secs: 0.0,
        ..Config::default()
    };
    let (outcome, messages, quit) = gh_on(
        &clock,
        &fake,
        &dir,
        &short,
        &strings(&["pr", "comment", "1", "--body", "hi"]),
    );
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(!quit, "only a credential call prints quit=1");
    let text = messages.join("\n");
    assert!(text.contains("cooldown after GitHub pushback"), "{text}");
    assert!(text.contains("beyond GH_PACED_MAX_WAIT=60 s"), "{text}");
    assert!(!text.contains("GH_PACED_GIT_MAX_WAIT"), "{text}");
    assert!(messages.len() > 1, "the full banner: {messages:?}");
    assert_eq!(fake.runs().len(), runs);
    let zero_git = Config {
        git_credential_max_wait_secs: 0.0,
        ..Config::default()
    };
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &zero_git, &get_args(1));
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert!(!quit);
    assert!(
        clock.sleeps().iter().any(|s| (s - 100.4).abs() < 1e-6),
        "the READ slept the rest of the cooldown: {:?}",
        clock.sleeps()
    );
    assert!(messages
        .join("\n")
        .contains("cooldown after GitHub pushback"));
    let ran = fake.runs();
    assert_eq!(ran.len(), runs + 1);
    assert!(ran[runs].at >= until, "{ran:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pause after state recovery is a cooldown too: a credential call is refused at once.
#[test]
fn credential_get_is_refused_at_once_during_the_recovery_pause() {
    let dir = scratch("cred-recovery");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    std::fs::write(dir.join("replay.json"), b"{\"buckets\": {\"read\"").unwrap();
    let cfg = Config::default();
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(quit);
    assert!(clock.sleeps().is_empty(), "slept: {:?}", clock.sleeps());
    assert!(fake.runs().is_empty());
    let refused: Vec<&String> = messages.iter().filter(|m| m.contains("REFUSED")).collect();
    assert_eq!(refused.len(), 1, "{messages:?}");
    assert!(
        refused[0].contains("pause after state recovery"),
        "{refused:?}"
    );
    assert!(refused[0].contains("900 s left"), "{refused:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cooldown with no end time is refused at once too, and the line says what to remove.
#[test]
fn credential_get_names_a_cooldown_with_no_end() {
    let dir = scratch("cred-no-end");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let until = start_cooldown(
        &clock,
        &fake,
        &dir,
        b"HTTP 429: Too Many Requests\nRetry-After: 99999999999\n",
    );
    assert!(gh_paced::pushback::has_no_end(until), "{until}");
    let (outcome, messages, quit) =
        gh_on(&clock, &fake, &dir, &Config::default(), &credential_get());
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(quit);
    assert!(clock.sleeps().is_empty(), "slept: {:?}", clock.sleeps());
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("the cooldown has no end time"),
        "{messages:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ordinary GIT_CREDENTIAL budget throttling may sleep, but never past GH_PACED_GIT_MAX_WAIT
/// (30 s), and a smaller GH_PACED_MAX_WAIT still wins.
#[test]
fn credential_get_waits_for_its_budget_at_most_the_short_bound() {
    let dir = scratch("cred-budget");
    let clock = FakeClock::new(T0);
    let fake = FakeGh::new(&clock);
    let cfg = Config::default();
    assert_eq!(cfg.git_credential_max_wait_secs, 30.0);
    let (outcome, messages, _) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert!(clock.sleeps().is_empty());
    // 1 per 10 s: the next call sleeps for its token, inside the bound.
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    assert!(!quit);
    let sleeps = clock.sleeps();
    assert_eq!(sleeps.len(), 1, "{sleeps:?}");
    assert!((9.0..=10.0).contains(&sleeps[0]), "{sleeps:?}");
    // At 1 per minute the token is ~59 s away: refused at once instead of sleeping.
    let slow = ClassLimits {
        per_minute: 1.0,
        burst: 1.0,
        per_hour: 120,
    };
    let slow_cfg = Config {
        git_credential: slow,
        ..Config::default()
    };
    let before = clock.now();
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &slow_cfg, &credential_get());
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(quit, "git is told to quit");
    assert_eq!(
        clock.sleeps().len(),
        1,
        "no new sleep: {:?}",
        clock.sleeps()
    );
    assert_eq!(clock.now(), before);
    assert_eq!(fake.runs().len(), 2);
    let text = messages.join("\n");
    assert!(text.contains("beyond GH_PACED_GIT_MAX_WAIT=30 s"), "{text}");
    assert!(text.contains("git_credential budget 1 per 60 s"), "{text}");
    // The bound is what refused it: raised to 900 s, the same call sleeps for its token.
    let patient = Config {
        git_credential_max_wait_secs: 900.0,
        ..slow_cfg
    };
    let (outcome, messages, _) = gh_on(&clock, &fake, &dir, &patient, &credential_get());
    assert_eq!(outcome, Outcome::Exit(0), "{messages:?}");
    let last = *clock.sleeps().last().unwrap();
    assert!(last > 30.0 && last <= 60.0, "{:?}", clock.sleeps());
    // A GH_PACED_MAX_WAIT below the credential bound applies and is the one named.
    let tight = Config {
        max_wait_secs: 5.0,
        ..Config::default()
    };
    let (outcome, messages, quit) = gh_on(&clock, &fake, &dir, &tight, &credential_get());
    assert_eq!(outcome, Outcome::Exit(EXIT_REFUSED), "{messages:?}");
    assert!(quit);
    assert!(
        messages.join("\n").contains("beyond GH_PACED_MAX_WAIT=5 s"),
        "{messages:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A clock whose every sleep is followed by another process taking the GIT_CREDENTIAL token
/// (and, optionally, by `late` more seconds before this process gets the state lock back).
struct ContendedClock<'a> {
    inner: &'a FakeClock,
    late: f64,
    slept: Mutex<Vec<f64>>,
    rival: Box<dyn Fn() + Send + Sync + 'a>,
}

impl Clock for ContendedClock<'_> {
    fn now(&self) -> f64 {
        self.inner.now()
    }

    fn sleep(&self, secs: f64) {
        self.slept.lock().unwrap().push(secs);
        self.inner.advance_to(self.inner.now() + secs);
        (self.rival)();
        self.inner.advance_to(self.inner.now() + self.late);
    }
}

/// A credential call that keeps losing its token to other processes gives up within the
/// bound, counting the time it spent not sleeping (waiting for the state lock) as well.
#[test]
fn contended_credential_get_gives_up_within_the_bound() {
    for late in [0.0, 8.0] {
        let dir = scratch(&format!("cred-contended-{late}"));
        let clock = FakeClock::new(T0);
        let fake = FakeGh::new(&clock);
        let cfg = Config::default();
        let (outcome, _, _) = gh_on(&clock, &fake, &dir, &cfg, &credential_get());
        assert_eq!(outcome, Outcome::Exit(0));
        let rivals = AtomicU64::new(0);
        let contended = ContendedClock {
            inner: &clock,
            late,
            slept: Mutex::new(Vec::new()),
            rival: Box::new(|| {
                let (outcome, messages) = gh(&clock, &fake, &dir, &cfg, &credential_get());
                assert_eq!(outcome, Outcome::Exit(0), "rival: {messages:?}");
                rivals.fetch_add(1, Ordering::SeqCst);
            }),
        };
        let start = clock.now();
        let (outcome, messages, quit) = gh_on(&contended, &fake, &dir, &cfg, &credential_get());
        let elapsed = clock.now() - start;
        assert_eq!(
            outcome,
            Outcome::Exit(EXIT_REFUSED),
            "late {late}: {messages:?}"
        );
        assert!(quit);
        let slept: f64 = contended.slept.lock().unwrap().iter().sum();
        assert!(slept <= 30.0, "late {late}: slept {slept}");
        assert!(
            rivals.load(Ordering::SeqCst) >= 2,
            "late {late}: the bound was reached through repeated waits"
        );
        // At most the bound plus one wake-up (a rival's call and the late lock).
        assert!(
            elapsed <= 30.0 + CALL_SECS + late + 1e-6,
            "late {late}: stayed {elapsed} s"
        );
        assert!(
            messages
                .join("\n")
                .contains("beyond GH_PACED_GIT_MAX_WAIT=30 s"),
            "late {late}: {messages:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

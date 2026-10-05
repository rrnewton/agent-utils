//! End-to-end tests of the built `gh-paced` binary against a fake `gh` shell script.
//!
//! These run real processes on the real clock, so the budgets come from a per-test config file
//! with short intervals (1 to 2 s) and the assertions use generous margins. The decision logic is
//! covered with a fake clock in `replay.rs` and the unit tests; this file checks what only real
//! processes can show: the shared lock across processes, pass-through of exit codes, output,
//! stdin and signals, and the command-line surface.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Instant;

const BIN: &str = env!("CARGO_BIN_EXE_gh-paced");

const FAKE_GH: &str = r#"#!/bin/bash
# Fake gh for gh-paced tests. Logs each call with a nanosecond timestamp.
log="${FAKE_GH_LOG:?}"
if [ "$1 $2" = "api rate_limit" ] && [ $# -eq 2 ]; then
  echo "$(date +%s.%N) rate_limit" >> "$log"
  if [ -n "${FAKE_GH_RATE_JSON:-}" ]; then printf '%s\n' "$FAKE_GH_RATE_JSON"; else
    r=$(( $(date +%s) + 3000 ))
    printf '{"resources":{"core":{"limit":5000,"used":10,"remaining":4990,"reset":%s},"graphql":{"limit":5000,"used":0,"remaining":5000,"reset":%s},"search":{"limit":30,"used":0,"remaining":30,"reset":%s}}}\n' "$r" "$r" "$r"
  fi
  exit 0
fi
echo "$(date +%s.%N) start $*" >> "$log"
if [ -n "${FAKE_GH_SIGSTATUS:-}" ]; then grep -E '^Sig(Blk|Ign)' /proc/$$/status > "$FAKE_GH_SIGSTATUS"; fi
if [ -n "${FAKE_GH_STDIN_FILE:-}" ]; then cat > "$FAKE_GH_STDIN_FILE"; fi
if [ -n "${FAKE_GH_NESTED:-}" ]; then
  # Run a nested gh-paced write from inside this call, as a gh alias or extension would.
  n="$FAKE_GH_NESTED"
  FAKE_GH_NESTED= FAKE_GH_SLEEP= "$n" --account test -- issue comment 99 --body nested
  echo "$(date +%s.%N) nested-rc $?" >> "$log"
fi
if [ -n "${FAKE_GH_SLEEP:-}" ]; then sleep "$FAKE_GH_SLEEP"; fi
if [ -n "${FAKE_GH_STDOUT:-}" ]; then printf '%s\n' "$FAKE_GH_STDOUT"; fi
if [ -n "${FAKE_GH_STDERR:-}" ]; then printf '%s\n' "$FAKE_GH_STDERR" >&2; fi
echo "$(date +%s.%N) end $*" >> "$log"
if [ -n "${FAKE_GH_SIGNAL:-}" ]; then kill -s "$FAKE_GH_SIGNAL" $$; sleep 5; fi
exit "${FAKE_GH_EXIT:-0}"
"#;

/// A scratch directory with a fake gh, a state directory and a config file.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gh-paced-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let gh = dir.join("gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("config.json"), config).unwrap();
        std::fs::write(dir.join("gh.log"), "").unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// `gh-paced --account test -- <args>` with a clean environment.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path("home"))
            .env("GH_PACED_REAL_GH", self.path("gh"))
            .env("GH_PACED_STATE_DIR", self.path("state"))
            .env("GH_PACED_CONFIG", self.path("config.json"))
            .env("FAKE_GH_LOG", self.path("gh.log"))
            .args(["--account", "test", "--"])
            .args(args)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = self.cmd(args);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    /// `(time, event, args)` lines from the fake gh log.
    fn log(&self) -> Vec<(f64, String, String)> {
        std::fs::read_to_string(self.path("gh.log"))
            .unwrap()
            .lines()
            .map(|l| {
                let mut it = l.splitn(3, ' ');
                let t = it.next().unwrap().parse().unwrap();
                let ev = it.next().unwrap_or("").to_string();
                (t, ev, it.next().unwrap_or("").to_string())
            })
            .collect()
    }

    fn starts(&self) -> Vec<(f64, String)> {
        self.log()
            .into_iter()
            .filter(|(_, ev, _)| ev == "start")
            .map(|(t, _, a)| (t, a))
            .collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Writes 1 per second (burst 1); reads 60/min with burst 10.
const FAST: &str = r#"{"write": {"per_minute": 60, "burst": 1, "per_hour": 30, "max_in_flight": 1},
                      "read": {"per_minute": 60, "burst": 10, "per_hour": 500}}"#;

#[test]
fn exit_code_stdout_and_stderr_pass_through() {
    let sb = Sandbox::new("exit", FAST);
    let o = sb.run(
        &["pr", "view", "5"],
        &[
            ("FAKE_GH_EXIT", "3"),
            ("FAKE_GH_STDOUT", "hello out"),
            ("FAKE_GH_STDERR", "hello err"),
        ],
    );
    assert_eq!(o.status.code(), Some(3), "{}", stderr(&o));
    assert_eq!(stdout(&o), "hello out\n");
    assert!(stderr(&o).contains("hello err"), "{}", stderr(&o));
    assert!(
        !stderr(&o).contains("GH-PACED"),
        "a quiet call prints nothing extra: {}",
        stderr(&o)
    );
}

#[test]
fn signal_death_is_reproduced() {
    let sb = Sandbox::new("signal", FAST);
    let o = sb.run(&["pr", "view", "5"], &[("FAKE_GH_SIGNAL", "TERM")]);
    assert_eq!(
        o.status.signal(),
        Some(libc::SIGTERM),
        "{:?} {}",
        o.status,
        stderr(&o)
    );
}

/// `(blocked, ignored)` signal masks the fake gh saw.
fn child_masks(path: &std::path::Path) -> (u64, u64) {
    let text = std::fs::read_to_string(path).unwrap();
    let field = |name: &str| {
        let line = text.lines().find(|l| l.starts_with(name)).unwrap();
        u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap()
    };
    (field("SigBlk:"), field("SigIgn:"))
}

fn bit(sig: i32) -> u64 {
    1u64 << (sig - 1)
}

#[test]
fn child_gets_default_signal_handling_and_inherits_ignored_signals() {
    let sb = Sandbox::new("sigmask", FAST);
    let status = sb.path("sig.txt");
    let status_s = status.display().to_string();
    let o = sb.run(&["pr", "view", "5"], &[("FAKE_GH_SIGSTATUS", &status_s)]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let (blocked, ignored) = child_masks(&status);
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        assert_eq!(
            blocked & bit(sig),
            0,
            "signal {sig} blocked in gh: {blocked:x}"
        );
        assert_eq!(
            ignored & bit(sig),
            0,
            "signal {sig} ignored in gh: {ignored:x}"
        );
    }
    // Started like `nohup gh-paced ...`: SIGHUP ignored, so gh must inherit that too.
    let mut cmd = sb.cmd(&["pr", "view", "6"]);
    cmd.env("FAKE_GH_SIGSTATUS", &status_s);
    // SAFETY: signal() is async-signal-safe; this runs in the forked child before exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let o = cmd.output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let (blocked, ignored) = child_masks(&status);
    assert_ne!(
        ignored & bit(libc::SIGHUP),
        0,
        "SIGHUP no longer ignored in gh: {ignored:x}"
    );
    assert_eq!(blocked & bit(libc::SIGTERM), 0);
}

#[test]
fn stdin_reaches_gh_untouched() {
    let sb = Sandbox::new("stdin", FAST);
    let got = sb.path("stdin.out");
    let got_s = got.display().to_string();
    // A read has no body to inspect, so gh inherits stdin directly. The input is far larger than
    // the guard's read limit (max_body_bytes + 1), so a buffered copy would arrive truncated.
    let big: Vec<u8> = (0..100_000u32)
        .map(|i| b"abcdefgh\n"[(i % 9) as usize])
        .collect();
    let mut child = sb
        .cmd(&["api", "-X", "GET", "repos/o/r/issues", "--input", "-"])
        .env("FAKE_GH_STDIN_FILE", &got_s)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let writer = {
        let big = big.clone();
        std::thread::spawn(move || input.write_all(&big))
    };
    let o = child.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        std::fs::read(&got).unwrap(),
        big,
        "read input arrived whole"
    );
    // A GraphQL request on stdin may be a mutation, so it is classified WRITE (fail safe): the
    // body is buffered for the content guard and then replayed byte for byte.
    let query = b"{\"query\":\"query { viewer { login } }\"}";
    let mut child = sb
        .cmd(&["api", "graphql", "--input", "-"])
        .env("FAKE_GH_STDIN_FILE", &got_s)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(query).unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(std::fs::read(&got).unwrap(), query);
    // A write body on stdin is buffered the same way.
    let body = b"{\"body\":\"a short prose note\"}\n";
    let mut child = sb
        .cmd(&[
            "api",
            "-X",
            "POST",
            "repos/o/r/issues/1/comments",
            "--input",
            "-",
        ])
        .env("FAKE_GH_STDIN_FILE", &got_s)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(body).unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(std::fs::read(&got).unwrap(), body);
    // Both buffered bodies were charged as writes.
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.path("state/test.json")).unwrap())
            .unwrap();
    assert_eq!(
        st["buckets"]["write"]["window"].as_array().unwrap().len(),
        2,
        "{st}"
    );
}

#[test]
fn content_guard_refuses_before_gh_runs() {
    let sb = Sandbox::new("guard", FAST);
    let encoded: String = (0..3000).map(|i| ["Ab3", "x9Z", "Qq7"][i % 3]).collect();
    let request = sb.path("part.request.json");
    std::fs::write(&request, serde_json::json!({ "body": encoded }).to_string()).unwrap();
    let request = request.display().to_string();
    let o = sb.run(
        &[
            "api",
            "--method",
            "POST",
            "repos/o/r/issues/1/comments",
            "--input",
            &request,
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(65), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(
        err.contains("GH-PACED REFUSED [test] content guard"),
        "{err}"
    );
    assert!(err.contains("over the 8192-byte limit"), "{err}");
    assert!(err.contains("base64-looking run"), "{err}");
    let o = sb.run(
        &["pr", "comment", "1", "--body", &"word ".repeat(2000)],
        &[],
    );
    assert_eq!(o.status.code(), Some(65), "{}", stderr(&o));
    assert!(sb.starts().is_empty(), "gh never ran: {:?}", sb.log());
    // The audit log records the refusal without the body.
    let audit = std::fs::read_to_string(sb.path("state/test.audit.jsonl")).unwrap();
    assert!(audit.contains("\"refuse\""), "{audit}");
    assert!(!audit.contains("Ab3x9Z"), "audit must not hold bodies");
    assert!(!audit.contains("word word"), "audit must not hold bodies");
    // An explicit override lets it through, loudly.
    let o = sb.run(
        &["pr", "comment", "1", "--body", &"word ".repeat(2000)],
        &[("GH_PACED_ALLOW_LARGE_BODY", "1")],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("content guard skipped"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn concurrent_processes_share_one_write_budget() {
    let sb = Sandbox::new("shared", FAST);
    let begun = Instant::now();
    let children: Vec<_> = (0..3)
        .map(|i| {
            sb.cmd(&["issue", "comment", &format!("{i}"), "--body", "short note"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    }
    let mut starts: Vec<f64> = sb.starts().iter().map(|(t, _)| *t).collect();
    starts.sort_by(f64::total_cmp);
    assert_eq!(starts.len(), 3);
    // The fake gh's own timestamps include bash start-up jitter, so the spacing is checked on
    // the admission times gh-paced recorded under the shared lock.
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.path("state/test.json")).unwrap())
            .unwrap();
    let mut admitted: Vec<f64> = st["buckets"]["write"]["window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e[0].as_f64().unwrap())
        .collect();
    admitted.sort_by(f64::total_cmp);
    assert_eq!(admitted.len(), 3, "{st}");
    for pair in admitted.windows(2) {
        assert!(
            pair[1] - pair[0] >= 1.0 - 1e-6,
            "admitted {} s apart: {admitted:?}",
            pair[1] - pair[0]
        );
    }
    assert!(
        starts[2] - starts[0] >= 1.8,
        "gh runs spread over {} s",
        starts[2] - starts[0]
    );
    assert!(begun.elapsed().as_secs_f64() >= 1.8);
    assert_eq!(
        st["in_flight"].as_array().unwrap().len(),
        0,
        "holders released: {st}"
    );
}

#[test]
fn only_one_write_in_flight_per_account() {
    // Bucket allows 5 at once; the in-flight limit must still serialize them.
    let cfg = r#"{"write": {"per_minute": 60, "burst": 5, "per_hour": 30, "max_in_flight": 1}}"#;
    let sb = Sandbox::new("inflight", cfg);
    let children: Vec<_> = (0..2)
        .map(|i| {
            sb.cmd(&["issue", "comment", &format!("{i}"), "--body", "short note"])
                .env("FAKE_GH_SLEEP", "1.5")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();
    for o in &outputs {
        assert_eq!(o.status.code(), Some(0), "{}", stderr(o));
    }
    let log: Vec<(f64, String)> = sb
        .log()
        .into_iter()
        .filter(|(_, ev, _)| ev == "start" || ev == "end")
        .map(|(t, ev, _)| (t, ev))
        .collect();
    let mut sorted = log.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let events: Vec<&str> = sorted.iter().map(|(_, e)| e.as_str()).collect();
    assert_eq!(
        events,
        ["start", "end", "start", "end"],
        "overlapping writes: {sorted:?}"
    );
    let waited = outputs
        .iter()
        .any(|o| stderr(o).contains("1 write(s) already in flight on this host"));
    assert!(
        waited,
        "the second writer announced the wait: {:?}",
        outputs.iter().map(stderr).collect::<Vec<_>>()
    );
}

#[test]
fn sleeps_within_the_bound_and_refuses_beyond_it() {
    let cfg = r#"{"write": {"per_minute": 30, "burst": 1, "per_hour": 30, "max_in_flight": 1}}"#;
    let sb = Sandbox::new("bound", cfg);
    let args = ["pr", "comment", "1", "--body", "short note"];
    let o = sb.run(&args, &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // Next slot is ~2 s away: a 1 s bound refuses without running gh.
    let o = sb.run(&args, &[("GH_PACED_MAX_WAIT", "1")]);
    assert_eq!(o.status.code(), Some(75), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(
        err.contains("GH-PACED REFUSED [test] write budget 30/min (burst 1) per host reached"),
        "{err}"
    );
    assert!(err.contains("beyond GH_PACED_MAX_WAIT=1 s"), "{err}");
    assert_eq!(sb.starts().len(), 1);
    // The default bound sleeps, warns, then runs.
    let begun = Instant::now();
    let o = sb.run(&args, &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(
        err.contains("GH-PACED WARNING [test] write budget 30/min (burst 1) per host reached"),
        "{err}"
    );
    assert!(err.contains("sleeping") && err.contains(" ET)"), "{err}");
    assert!(begun.elapsed().as_secs_f64() >= 0.5, "it slept");
    assert_eq!(sb.starts().len(), 2);
}

#[test]
fn github_pushback_starts_a_cooldown() {
    let sb = Sandbox::new("pushback", FAST);
    let o = sb.run(
        &["pr", "list"],
        &[
            ("FAKE_GH_STDERR", "HTTP 403: You have exceeded a secondary rate limit and have been temporarily blocked"),
            ("FAKE_GH_EXIT", "1"),
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(1),
        "gh's status passes through: {}",
        stderr(&o)
    );
    let err = stderr(&o);
    assert!(
        err.contains("secondary rate limit and have been"),
        "gh's own stderr is shown: {err}"
    );
    assert!(err.contains("GH-PACED PUSHBACK [test]"), "{err}");
    assert!(err.contains("Never switch accounts"), "{err}");
    for args in [
        &["pr", "view", "1"][..],
        &["pr", "comment", "1", "--body", "x"],
        &["auth", "git-credential", "get"],
    ] {
        let o = sb.run(args, &[("GH_PACED_MAX_WAIT", "5")]);
        assert_eq!(o.status.code(), Some(75), "{args:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("cooldown"), "{}", stderr(&o));
    }
    assert_eq!(sb.starts().len(), 1, "{:?}", sb.log());
    // Status shows the cooldown.
    let o = Command::new(BIN)
        .env_clear()
        .env("HOME", sb.path("home"))
        .env("GH_PACED_STATE_DIR", sb.path("state"))
        .env("GH_PACED_CONFIG", sb.path("config.json"))
        .args(["status", "--account", "test"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).contains("COOLDOWN:"), "{}", stdout(&o));
}

#[test]
fn local_commands_run_without_touching_state() {
    let sb = Sandbox::new("local", FAST);
    let o = sb.run(&["--version"], &[("FAKE_GH_STDOUT", "gh version 9.9.9")]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o), "gh version 9.9.9\n");
    let entries: Vec<_> = std::fs::read_dir(sb.path("state")).unwrap().collect();
    assert!(entries.is_empty(), "LOCAL left state behind: {entries:?}");
}

#[test]
fn surface_errors_have_distinct_exit_codes() {
    let sb = Sandbox::new("errors", FAST);
    // No account.
    let o = Command::new(BIN)
        .env_clear()
        .env("GH_PACED_REAL_GH", sb.path("gh"))
        .env("GH_PACED_STATE_DIR", sb.path("state"))
        .args(["--", "pr", "list"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64), "{}", stderr(&o));
    // The "real gh" is gh-paced itself: refused instead of recursing.
    let o = sb.run(&["pr", "list"], &[("GH_PACED_REAL_GH", BIN)]);
    assert_eq!(o.status.code(), Some(78), "{}", stderr(&o));
    // Nesting depth.
    let o = sb.run(&["pr", "list"], &[("GH_PACED_DEPTH", "8")]);
    assert_eq!(o.status.code(), Some(75), "{}", stderr(&o));
    // A loosening env override is ignored with a warning, a malformed one is a config error.
    let o = sb.run(&["pr", "list"], &[("GH_PACED_WRITE_PER_HOUR", "lots")]);
    assert_eq!(o.status.code(), Some(78), "{}", stderr(&o));
    assert!(sb.starts().is_empty());
}

#[test]
fn help_classify_and_status_subcommands() {
    let sb = Sandbox::new("help", FAST);
    for args in [
        &["--help"][..],
        &["status", "--help"],
        &["classify", "--help"],
    ] {
        let o = Command::new(BIN).env_clear().args(args).output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        let text = stdout(&o);
        assert!(
            text.contains("EXAMPLES") || text.contains("Examples"),
            "{args:?}: {text}"
        );
    }
    let o = Command::new(BIN).env_clear().output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(64),
        "no arguments prints help and fails"
    );
    let o = Command::new(BIN)
        .env_clear()
        .env("HOME", sb.path("home"))
        .env("GH_PACED_CONFIG", sb.path("config.json"))
        .args([
            "classify",
            "--json",
            "--",
            "api",
            "-X",
            "PATCH",
            "repos/o/r/issues/1",
            "-f",
            "state=closed",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["class"], "write");
    assert!(sb.starts().is_empty(), "classify never runs gh");
    // One read, then status reports it.
    let o = sb.run(&["pr", "view", "1"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let o = Command::new(BIN)
        .env_clear()
        .env("HOME", sb.path("home"))
        .env("GH_PACED_STATE_DIR", sb.path("state"))
        .env("GH_PACED_CONFIG", sb.path("config.json"))
        .args(["status", "--account", "test", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v["classes"]["read"]["hour_used"], 2,
        "1 call + 1 rate_limit refresh: {v}"
    );
    assert_eq!(
        v["rate_limit"]["resources"]["core"]["remaining"], 4990,
        "{v}"
    );
}

/// `status` reads a state directory it cannot write to, creates no lock file there, and creates
/// no state directory for an account that has none.
#[test]
fn status_takes_no_lock_and_creates_nothing() {
    let sb = Sandbox::new("status-ro", FAST);
    let o = sb.run(&["pr", "view", "1"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let state = sb.path("state");
    let lock = state.join("test.lock");
    std::fs::remove_file(&lock).expect("the paced call created the lock file");
    let status = |dir: &std::path::Path| {
        Command::new(BIN)
            .env_clear()
            .env("HOME", sb.path("home"))
            .env("GH_PACED_STATE_DIR", dir)
            .env("GH_PACED_CONFIG", sb.path("config.json"))
            .args(["status", "--account", "test", "--json"])
            .output()
            .unwrap()
    };
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o500)).unwrap();
    let o = status(&state);
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["classes"]["read"]["hour_used"], 2, "{v}");
    assert!(!lock.exists(), "status created {}", lock.display());
    let missing = sb.path("no-such-state");
    let o = status(&missing);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!missing.exists(), "status created {}", missing.display());
}

/// Secrets in an endpoint's query string reach gh unchanged but never the audit log, the state
/// file or gh-paced's own messages.
#[test]
fn query_string_secrets_stay_out_of_the_records() {
    let sb = Sandbox::new("canary", FAST);
    let o = sb.run(
        &[
            "api",
            "repos/o/r/issues?access_token=github_pat_CANARY_READ",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let o2 = sb.run(
        &[
            "api",
            "-X",
            "POST",
            "https://api.github.com/repos/o/r/issues?access_token=github_pat_CANARY_WRITE",
            "-f",
            "title=short",
        ],
        &[],
    );
    assert_eq!(o2.status.code(), Some(0), "{}", stderr(&o2));
    // An alias or extension: its positional arguments may be a body or a secret.
    let o3 = sb.run(
        &["my-alias", "CANARY_ALIAS secret body", "--flag=CANARY_FLAG"],
        &[],
    );
    assert_eq!(o3.status.code(), Some(0), "{}", stderr(&o3));
    let log = std::fs::read_to_string(sb.path("gh.log")).unwrap();
    assert!(
        log.contains("CANARY_ALIAS") && log.contains("CANARY_FLAG"),
        "{log}"
    );
    assert!(
        log.contains("CANARY_READ") && log.contains("CANARY_WRITE"),
        "gh got the real args: {log}"
    );
    let audit = std::fs::read_to_string(sb.path("state/test.audit.jsonl")).unwrap();
    assert!(audit.contains("\"admit\""), "{audit}");
    let state = std::fs::read_to_string(sb.path("state/test.json")).unwrap();
    for (what, text) in [
        ("audit", audit.as_str()),
        ("state", state.as_str()),
        ("stderr", &stderr(&o)),
        ("stderr", &stderr(&o2)),
        ("stderr", &stderr(&o3)),
    ] {
        assert!(!text.contains("CANARY"), "{what} holds a secret: {text}");
        assert!(
            !text.contains("access_token"),
            "{what} holds the query: {text}"
        );
    }
}

fn wait_for_log(sb: &Sandbox, needle: &str) {
    let begun = Instant::now();
    while !std::fs::read_to_string(sb.path("gh.log"))
        .unwrap()
        .contains(needle)
    {
        assert!(
            begun.elapsed().as_secs_f64() < 15.0,
            "never saw {needle:?}: {:?}",
            sb.log()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Time of the first `event` line whose arguments start with `args`.
fn event_time(sb: &Sandbox, event: &str, args: &str) -> f64 {
    sb.log()
        .into_iter()
        .find(|(_, ev, a)| ev == event && a.starts_with(args))
        .map(|(t, _, _)| t)
        .unwrap_or_else(|| panic!("no {event} {args}: {:?}", sb.log()))
}

fn lease_files(sb: &Sandbox) -> Vec<String> {
    std::fs::read_dir(sb.path("state"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".lease-"))
        .collect()
}

/// Five writes may start at once by the bucket; only the in-flight limit separates them.
const BURSTY: &str =
    r#"{"write": {"per_minute": 60, "burst": 5, "per_hour": 30, "max_in_flight": 1}}"#;

/// A write's slot lasts as long as gh does, not as long as the wrapper: when the wrapper is killed
/// with SIGKILL its orphaned gh keeps the lease file locked, so the next write waits for gh.
#[test]
fn killed_wrapper_leaves_its_orphaned_gh_holding_the_write_slot() {
    let sb = Sandbox::new("orphan", BURSTY);
    let mut first = sb
        .cmd(&["issue", "comment", "1", "--body", "orphaned"])
        .env("FAKE_GH_SLEEP", "3")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_log(&sb, "start issue comment 1");
    first.kill().unwrap();
    first.wait().unwrap();
    assert_eq!(lease_files(&sb).len(), 1, "the lease outlives the wrapper");
    let o = sb.run(&["issue", "comment", "2", "--body", "next"], &[]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let orphan_end = event_time(&sb, "end", "issue comment 1");
    let next_start = event_time(&sb, "start", "issue comment 2");
    assert!(
        next_start >= orphan_end,
        "second write started {} s before the orphan finished",
        orphan_end - next_start
    );
    assert!(
        stderr(&o).contains("1 write(s) already in flight on this host"),
        "{}",
        stderr(&o)
    );
    assert!(lease_files(&sb).is_empty(), "{:?}", lease_files(&sb));
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.path("state/test.json")).unwrap())
            .unwrap();
    assert_eq!(st["in_flight"].as_array().unwrap().len(), 0, "{st}");
}

/// A gh-paced started by gh itself (an alias or extension) skips the in-flight wait for its
/// parent's write, because it holds the parent's lease file descriptor. Naming the parent's
/// nonce in `GH_PACED_INFLIGHT_CHAIN` without that descriptor is not enough: such a process
/// waits like any other.
#[test]
fn nesting_needs_the_inherited_lease_not_just_the_chain() {
    let sb = Sandbox::new("nested", BURSTY);
    let outer = sb
        .cmd(&["issue", "comment", "1", "--body", "outer"])
        .env("FAKE_GH_NESTED", BIN)
        .env("FAKE_GH_SLEEP", "3")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_log(&sb, "nested-rc");
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.path("state/test.json")).unwrap())
            .unwrap();
    let holders = st["in_flight"].as_array().unwrap();
    assert_eq!(holders.len(), 1, "only the outer write is in flight: {st}");
    let nonce = holders[0]["nonce"].as_str().unwrap().to_string();
    let forged = sb.run(
        &["issue", "comment", "2", "--body", "forged"],
        &[("GH_PACED_INFLIGHT_CHAIN", &nonce)],
    );
    let outer = outer.wait_with_output().unwrap();
    assert_eq!(outer.status.code(), Some(0), "{}", stderr(&outer));
    assert_eq!(forged.status.code(), Some(0), "{}", stderr(&forged));
    let log = sb.log();
    let nested_rc = log
        .iter()
        .find(|(_, ev, _)| ev == "nested-rc")
        .map(|(_, _, a)| a.clone());
    assert_eq!(nested_rc.as_deref(), Some("0"), "{log:?}");
    let outer_start = event_time(&sb, "start", "issue comment 1");
    let outer_end = event_time(&sb, "end", "issue comment 1");
    let nested_start = event_time(&sb, "start", "issue comment 99");
    assert!(
        nested_start > outer_start && nested_start < outer_end,
        "the nested write ran inside the outer one: {log:?}"
    );
    assert!(
        !stderr(&outer).contains("already in flight"),
        "the nested write did not wait: {}",
        stderr(&outer)
    );
    let forged_start = event_time(&sb, "start", "issue comment 2");
    assert!(
        forged_start >= outer_end,
        "the forged chain skipped the wait: {log:?}"
    );
    assert!(
        stderr(&forged).contains("1 write(s) already in flight on this host"),
        "{}",
        stderr(&forged)
    );
}

/// A watch loop is stopped once it has polled for as long as its up-front charge covers, and the
/// wrapper exits 75.
#[test]
fn watch_past_its_deadline_is_stopped() {
    let cfg = r#"{"watch_cost": 2, "min_watch_interval_secs": 0,
                  "read": {"per_minute": 60, "burst": 10, "per_hour": 500}}"#;
    let sb = Sandbox::new("deadline", cfg);
    let begun = Instant::now();
    // stdout goes to /dev/null: gh's stdout is inherited, and the fake gh's orphaned `sleep`
    // would hold a pipe open past the wrapper's exit.
    let o = sb
        .cmd(&["pr", "checks", "1", "--watch", "--interval", "1"])
        .env("FAKE_GH_SLEEP", "8")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let took = begun.elapsed().as_secs_f64();
    assert_eq!(o.status.code(), Some(75), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("GH-PACED DEADLINE [test]"), "{err}");
    assert!(err.contains("ran past its 2 s budget"), "{err}");
    assert!((1.9..7.5).contains(&took), "stopped after {took} s");
    assert!(
        !sb.log()
            .iter()
            .any(|(_, ev, a)| ev == "end" && a.starts_with("pr checks")),
        "gh was stopped before it finished: {:?}",
        sb.log()
    );
    let audit = std::fs::read_to_string(sb.path("state/test.audit.jsonl")).unwrap();
    assert!(audit.contains("\"deadline\""), "{audit}");
}

/// `gh api --include` prints the response headers on stdout. A Retry-After there sets the
/// cooldown, and stdout still passes through byte for byte. Without `--include` the same text is
/// a response body and is not read as headers.
#[test]
fn include_headers_on_stdout_set_the_cooldown() {
    let sb = Sandbox::new("include", FAST);
    let response = "HTTP/2.0 403 Forbidden\r\nRetry-After: 3600\r\nX-Ratelimit-Remaining: 0\r\n\r\n{\"message\":\"You have exceeded a secondary rate limit\"}";
    let env = [("FAKE_GH_STDOUT", response), ("FAKE_GH_EXIT", "1")];
    let state = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(sb.path("state/test.json")).unwrap()).unwrap()
    };
    let o = sb.run(&["api", "repos/o/r/issues"], &env);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(stdout(&o), format!("{response}\n"));
    assert!(
        state()["cooldown"].is_null(),
        "body text is not headers: {}",
        state()
    );
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let o = sb.run(&["api", "-i", "repos/o/r/issues"], &env);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(stdout(&o), format!("{response}\n"), "stdout passes through");
    assert!(
        stderr(&o).contains("GH-PACED PUSHBACK [test]"),
        "{}",
        stderr(&o)
    );
    let until = state()["cooldown"]["until"].as_f64().unwrap();
    assert!(
        until - before >= 3599.0 && until - before < 3700.0,
        "cooldown {} s",
        until - before
    );
}

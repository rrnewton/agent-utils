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
    // A read inherits stdin directly.
    let mut child = sb
        .cmd(&["api", "graphql", "--input", "-"])
        .env("FAKE_GH_STDIN_FILE", &got_s)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"query\":\"query { viewer { login } }\"}")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        std::fs::read(&got).unwrap(),
        b"{\"query\":\"query { viewer { login } }\"}"
    );
    // A write body on stdin is buffered for the content guard and then replayed byte for byte.
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

//! End-to-end bracket: a caller can tell boxed / skipped-by-policy / probe-failed apart.
//!
//! WHAT WENT WRONG WITHOUT THIS. `reexec_in_scope` returned `true` for BOTH "already boxed,
//! proceed" and "policy said skip, we never asked whether boxing was possible", and the caller
//! folded that bool into a single error whose wording, on the skip path, was "boxing was skipped
//! (e.g. CI without a systemd --user scope)". That sentence asserts a cause the code had not
//! tested. Four separate capability probes were then run against it — measuring a branch that
//! never executes in the environment under investigation.
//!
//! THE POLICY IS UNCHANGED HERE and these cases pin that down: every exit code below is what it
//! was before. What changed is that the run now says which of the four things happened, and offers
//! the instrument (`DAGRUN_FORCE_SCOPE_ATTEMPT=1`) to answer the capability question on a runner
//! population nobody has measured.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const DAG: &str =
    r#"{"steps":[{"group":"g","job":"j","cmd":"echo hi","timeout":30,"cpu_timeout":600}]}"#;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    /// `name` MUST be unique per test: cargo runs these concurrently in ONE process, so a
    /// pid-only directory name has every case deleting the fixture the others are reading. Two of
    /// these tests failed that way, intermittently, with a missing-DAG error that looked nothing
    /// like the thing under test.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("dagrun_scope_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.json"), DAG).unwrap();
        Fixture { dir }
    }
    fn dag(&self) -> PathBuf {
        self.dir.join("one.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn exact_process_is_present(pid: u32, start_time: u64) -> bool {
    process_start_time(pid) == Some(start_time)
}

struct Out {
    code: Option<i32>,
    text: String,
}

/// Build a runner command without inheriting the scope policy/control knobs that these tests vary.
///
/// In particular, GitHub Actions exports `GITHUB_ACTIONS=1` to the test process. Letting that leak
/// into a child which explicitly sets `CI=1` changes the selected policy reason, while letting it
/// leak into a child intended to exercise boxing changes the entire path into a policy skip. Keep
/// the ordinary process environment (notably `PATH`) but make these inputs explicit per case.
fn runner_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dagrun"));
    for key in [
        "CI",
        "GITHUB_ACTIONS",
        "DAGRUN_FORCE_SCOPE_ATTEMPT",
        "DAGRUN_IN_SCOPE",
        "DAGRUN_SCOPE_UNIT",
        "DAGRUN_DIRECT_CGROUP",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

/// Run the built binary with a controlled environment.
///
/// `hide_session` removes the two variables that make a systemd `--user` bus reachable, which is
/// how a runner with no user manager looks from inside the process.
fn run(fx: &Fixture, envs: &[(&str, &str)], extra: &[&str], hide_session: bool) -> Out {
    let mut cmd = runner_command();
    cmd.args(["run", "--dag", fx.dag().to_str().unwrap(), "--no-profile"])
        .args(extra)
        // Keep the evidence writer out of this test's way; it is covered elsewhere.
        .env("DAGRUN_NO_STEP_LOGS", "1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if hide_session {
        cmd.env_remove("XDG_RUNTIME_DIR")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
    }
    let out = cmd.output().expect("failed to spawn the built binary");
    Out {
        code: out.status.code(),
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

/// A: `CI` set, boxing required. The historical behaviour (exit 3) with an honest reason.
#[test]
fn policy_skip_is_named_as_untested_not_as_unavailable() {
    let fx = Fixture::new("policy_skip");
    let r = run(&fx, &[("CI", "1")], &[], false);

    assert_eq!(r.code, Some(3), "exit code must be unchanged:\n{}", r.text);
    assert!(
        r.text.contains("SKIPPED BY POLICY") && r.text.contains("$CI"),
        "the skip must name itself and the variable that selected it:\n{}",
        r.text
    );
    assert!(
        r.text.contains("NOT TESTED") || r.text.contains("NOT tested"),
        "the skip must say the capability question was never asked:\n{}",
        r.text
    );
    assert!(
        r.text.contains("DAGRUN_FORCE_SCOPE_ATTEMPT"),
        "and it must point at the instrument that would answer it:\n{}",
        r.text
    );
    // THE REGRESSION GUARD. This exact sentence sent an investigation after a capability the code
    // had not measured; a skip must never speak for the probe.
    assert!(
        !r.text
            .contains("boxing was skipped (e.g. CI without a systemd --user scope)"),
        "a policy skip must not assert a cause it never tested:\n{}",
        r.text
    );
}

/// B: forced attempt on a host with no reachable user bus — a REAL probe failure, distinct from A.
#[test]
fn a_forced_attempt_that_fails_reports_a_probe_failure_not_a_skip() {
    let fx = Fixture::new("forced_fail");
    let r = run(
        &fx,
        &[("CI", "1"), ("DAGRUN_FORCE_SCOPE_ATTEMPT", "1")],
        &[],
        true,
    );

    assert_eq!(
        r.code,
        Some(3),
        "an unavailable scope still refuses:\n{}",
        r.text
    );
    assert!(
        r.text.contains("attempted and failed"),
        "a probe that ran and failed must say so:\n{}",
        r.text
    );
    assert!(
        !r.text.contains("SKIPPED BY POLICY"),
        "forcing the attempt must actually lift the skip:\n{}",
        r.text
    );
}

/// C (control): `--allow-cgroup-failure` is unaffected — the sanctioned opt-out still runs unboxed.
///
/// This is the path every consuming CI lane actually takes, and it short-circuits ahead of the scope
/// logic entirely. If this changed, the finding would have cost a green CI to report.
#[test]
fn the_sanctioned_opt_out_is_untouched() {
    let fx = Fixture::new("opt_out");
    let r = run(&fx, &[("CI", "1")], &["--allow-cgroup-failure"], false);

    assert_eq!(r.code, Some(0), "the opt-out must still pass:\n{}", r.text);
    assert!(
        r.text.contains("UNBOXED"),
        "and must still say it is unboxed:\n{}",
        r.text
    );
    assert!(
        !r.text.contains("SKIPPED BY POLICY"),
        "the opt-out returns before the scope logic, so the skip must not even be reached:\n{}",
        r.text
    );
}

/// D: on a host that CAN box, forcing the attempt under `CI` actually boxes.
///
/// Skipped where a `--user` scope is unavailable, so the test says nothing false on a runner. This
/// is the positive half: without it, A and B are satisfiable by a build that never boxes at all.
#[test]
fn forcing_the_attempt_boxes_where_boxing_is_possible() {
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            &format!("--unit=dagrun-attempt-probe-{}", std::process::id()),
            "true",
        ])
        .output();
    let available = probe.map(|o| o.status.success()).unwrap_or(false);
    if !available {
        eprintln!("skipping: no usable systemd --user scope on this host");
        return;
    }

    let fx = Fixture::new("forced_box");
    let r = run(
        &fx,
        &[("CI", "1"), ("DAGRUN_FORCE_SCOPE_ATTEMPT", "1")],
        &[],
        false,
    );

    assert_eq!(
        r.code,
        Some(0),
        "a forced attempt on a capable host should box and pass:\n{}",
        r.text
    );
    assert!(
        r.text.contains("cgroup boxing ACTIVE"),
        "containment must actually be established, not merely claimed possible:\n{}",
        r.text
    );
}

/// PROOF, NOT DECLARATION: the boxed path must SEE the live pid inside the promised cgroup.
///
/// Skipped where a `--user` scope is unavailable, so this says nothing false on a runner.
#[test]
fn the_boxed_path_observes_the_live_pid_in_the_promised_cgroup() {
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            &format!("--unit=dagrun-proof-probe-{}", std::process::id()),
            "true",
        ])
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("skipping: no usable systemd --user scope on this host");
        return;
    }
    let fx = Fixture::new("proof");
    let r = run(&fx, &[], &[], false);

    assert_eq!(r.code, Some(0), "a boxed run should pass:\n{}", r.text);
    assert!(
        r.text.contains("containment OBSERVED: pid "),
        "the run must report an OBSERVED pid, not merely that boxing is ACTIVE:\n{}",
        r.text
    );
    // The observation must be bound to the cgroup that was actually arranged.
    assert!(
        r.text.contains("promised unit dagrun-") && r.text.contains("/sys/fs/cgroup/"),
        "the proof must name the promised unit and a real cgroup path:\n{}",
        r.text
    );
}

/// NEGATIVE: a FORGED in-scope sentinel must never be believed.
///
/// This is the exact lie the old code accepted — `is_in_scope()` read an environment variable, and
/// every consumer printed "cgroup boxing ACTIVE" on the strength of it. Anything can export it.
#[test]
fn a_forged_in_scope_sentinel_is_refused_not_believed() {
    let fx = Fixture::new("forged");
    let r = run(&fx, &[("DAGRUN_IN_SCOPE", "1")], &[], false);

    assert_eq!(
        r.code,
        Some(3),
        "a sentinel with no observable containment must refuse:\n{}",
        r.text
    );
    assert!(
        r.text.contains("could NOT be observed"),
        "the refusal must say the observation failed:\n{}",
        r.text
    );
    assert!(
        !r.text.contains("boxing ACTIVE"),
        "containment must NEVER be claimed on the strength of an environment variable:\n{}",
        r.text
    );
}

/// NEGATIVE: a sentinel pointing at a unit this process is not in must be refused, and the
/// refusal must name the cgroup actually observed — a mismatch is more useful than a bare "no".
#[test]
fn a_sentinel_naming_the_wrong_unit_is_refused_and_names_what_it_found() {
    let fx = Fixture::new("wrongunit");
    let r = run(
        &fx,
        &[
            ("DAGRUN_IN_SCOPE", "1"),
            ("DAGRUN_SCOPE_UNIT", "totally-not-our-unit.scope"),
        ],
        &[],
        false,
    );

    assert_eq!(
        r.code,
        Some(3),
        "a wrong-unit claim must refuse:\n{}",
        r.text
    );
    assert!(
        r.text
            .contains("the promised unit was totally-not-our-unit.scope"),
        "the refusal must name the promised unit it could not confirm:\n{}",
        r.text
    );
    assert!(
        r.text.contains("/sys/fs/cgroup/"),
        "and the cgroup it actually observed, so the mismatch is actionable:\n{}",
        r.text
    );
}

/// CONTROL: the forged sentinel plus the sanctioned opt-out degrades to UNBOXED and still passes.
///
/// Without this, the three refusals above are satisfiable by a build that refuses everything.
#[test]
fn a_forged_sentinel_with_the_opt_out_degrades_rather_than_claiming() {
    let fx = Fixture::new("forged_optout");
    let r = run(
        &fx,
        &[("DAGRUN_IN_SCOPE", "1")],
        &["--allow-cgroup-failure"],
        false,
    );

    assert_eq!(r.code, Some(0), "the opt-out must still pass:\n{}", r.text);
    assert!(
        r.text.contains("UNBOXED") && r.text.contains("could NOT be observed"),
        "it must degrade AND say why:\n{}",
        r.text
    );
    assert!(
        !r.text.contains("boxing ACTIVE"),
        "degrading must never print a containment claim:\n{}",
        r.text
    );
}

/// Read the containment record out of a run's durable journal.
fn journal_containment(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("journal.jsonl")).ok()?;
    text.lines()
        .find(|l| l.contains("\"event\":\"containment\""))
        .map(str::to_string)
}

/// Run once with an explicit evidence directory, returning (output, journal line).
fn run_recording(name: &str, envs: &[(&str, &str)], extra: &[&str]) -> (String, Option<String>) {
    let dir = std::env::temp_dir().join(format!("dagrun_rec_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dag = dir.join("one.json");
    std::fs::write(&dag, DAG).unwrap();
    let ev = dir.join("evidence");
    let mut cmd = runner_command();
    cmd.args(["run", "--dag", dag.to_str().unwrap(), "--no-profile"])
        .args(extra)
        .env("DAGRUN_LOG_DIR", &ev);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("failed to spawn the built binary");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let line = journal_containment(&ev);
    let _ = std::fs::remove_dir_all(&dir);
    (text, line)
}

/// THE RECORD, not a warning that scrolls past.
///
/// A run used to say ONCE, on stderr, that it was unboxed, and then no artifact carried it — so a
/// run reviewed later was indistinguishable from a boxed one. Establishing whether CI boxes at all
/// then cost four probes and three confident wrong answers, because the evidence had never
/// existed. Every state must reach the durable journal AND the banner line.
#[test]
fn every_containment_state_reaches_the_banner_and_the_durable_journal() {
    // 1) UNBOXED. The state that used to vanish.
    let (text, line) = run_recording("unboxed", &[("CI", "1")], &["--allow-cgroup-failure"]);
    assert!(
        text.contains("CONTAINMENT: unboxed"),
        "the banner must name the state:\n{text}"
    );
    let line = line.expect("an unboxed run must still write a containment record");
    assert!(
        line.contains("\"state\":\"unboxed\"")
            && line.contains("\"caps_enforced\":\"false\"")
            && line.contains("\"subtree_killable\":\"false\""),
        "the journal must carry the state and what it did NOT enforce:\n{line}"
    );

    // 2) STEPS-CONTAINED-ONLY must be its own state. Recording the direct route as "boxed" would
    //    be the overclaim relocated into an artifact that outlives the run.
    let (text, line) = run_recording("direct", &[("CI", "1"), ("DAGRUN_DIRECT_CGROUP", "1")], &[]);
    // GATE ON WHETHER THE ROUTE WAS TAKEN, NOT ON THE ANSWER IT GAVE. Keying the skip off the
    // state label means a build that mislabels the direct route as run-boxed simply skips this
    // case and passes — measured: it did. The route announces itself independently of the label,
    // so that announcement is what decides whether the assertion applies.
    let route_taken = text.contains("teardown ACTIVE via direct cgroupfs");
    if route_taken {
        assert!(
            text.contains("CONTAINMENT: steps-contained-only"),
            "the direct cgroupfs route must record steps-contained-only, never run-boxed:\n{text}"
        );
        let line = line.expect("the direct route must write a containment record");
        assert!(
            line.contains("\"state\":\"steps-contained-only\"")
                && line.contains("\"caps_enforced\":\"false\"")
                && line.contains("\"subtree_killable\":\"true\""),
            "steps-contained-only must record killable-but-uncapped, not boxed:\n{line}"
        );
        assert!(
            !line.contains("\"state\":\"run-boxed\""),
            "the direct route must never record itself as run-boxed:\n{line}"
        );
    } else {
        eprintln!("skipping direct-route case: cgroupfs route unavailable here");
    }

    // 3) RUN-BOXED, where the host can box. Carries the observation so the claim is recheckable.
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            &format!("--unit=dagrun-rec-probe-{}", std::process::id()),
            "true",
        ])
        .output();
    if probe.map(|o| o.status.success()).unwrap_or(false) {
        let (text, line) = run_recording("boxed", &[], &[]);
        assert!(
            text.contains("CONTAINMENT: run-boxed"),
            "a boxed run must say so in the banner:\n{text}"
        );
        let line = line.expect("a boxed run must write a containment record");
        assert!(
            line.contains("\"state\":\"run-boxed\"")
                && line.contains("\"caps_enforced\":\"true\"")
                && line.contains("observed in /sys/fs/cgroup/"),
            "the boxed record must carry the OBSERVED cgroup, not just the word boxed:\n{line}"
        );
    } else {
        eprintln!("skipping run-boxed case: no usable systemd --user scope on this host");
    }
}

/// Exercise the production direct-cgroup path with a real descendant that has left the leader's
/// process group. The release handshake makes the parent observe `populated 1` before the leader
/// exits and normal-exit teardown kills the matching payload.
#[test]
fn direct_cgroup_kills_matching_contained_payload_and_proves_empty() {
    let fixture = Fixture::new("direct_matching_kill");
    let pid_file = fixture.dir.join("payload.pid");
    let cgroup_file = fixture.dir.join("payload.cgroup");
    let release_file = fixture.dir.join("leader.release");
    let late_marker = fixture.dir.join("payload.late");
    let payload = format!(
        "echo $$ > {}; sed -n 's|^0::|/sys/fs/cgroup|p' /proc/self/cgroup > {}; \
         sleep 60; printf late > {}",
        shell_quote(&pid_file.to_string_lossy()),
        shell_quote(&cgroup_file.to_string_lossy()),
        shell_quote(&late_marker.to_string_lossy()),
    );
    let command = format!(
        "setsid /bin/sh -c {} </dev/null >/dev/null 2>&1 & \
         while [ ! -s {} ] || [ ! -s {} ]; do sleep 0.01; done; \
         while [ ! -e {} ]; do sleep 0.01; done",
        shell_quote(&payload),
        shell_quote(&pid_file.to_string_lossy()),
        shell_quote(&cgroup_file.to_string_lossy()),
        shell_quote(&release_file.to_string_lossy()),
    );
    let dag = fixture.dir.join("matching.json");
    let document = serde_json::json!({
        "steps": [{
            "group": "containment",
            "job": "matching",
            "cmd": command,
            "timeout": 30,
            "cpu_timeout": 600
        }]
    });
    std::fs::write(&dag, serde_json::to_vec(&document).unwrap()).unwrap();

    let mut runner = runner_command();
    runner
        .args([
            "run",
            "--dag",
            dag.to_str().unwrap(),
            "--max-steps",
            "1",
            "--no-profile",
        ])
        .env("CI", "1")
        .env("DAGRUN_DIRECT_CGROUP", "1")
        .env("DAGRUN_NO_STEP_LOGS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = runner.spawn().expect("failed to spawn dagrun");

    let deadline = Instant::now() + Duration::from_secs(5);
    while (!pid_file.is_file() || !cgroup_file.is_file()) && Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let observation = if pid_file.is_file() && cgroup_file.is_file() {
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let start_time = process_start_time(pid);
        let cgroup = PathBuf::from(std::fs::read_to_string(&cgroup_file).unwrap().trim());
        let populated = std::fs::read_to_string(cgroup.join("cgroup.events"))
            .ok()
            .is_some_and(|events| events.lines().any(|line| line == "populated 1"));
        Some((pid, start_time, cgroup, populated))
    } else {
        None
    };
    std::fs::write(&release_file, b"release\n").unwrap();

    let output = child.wait_with_output().expect("failed to wait for dagrun");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !text.contains("teardown ACTIVE via direct cgroupfs") {
        let explicit_unavailable = text.contains(
            "DAGRUN_DIRECT_CGROUP=1 but direct cgroupfs containment could not be established",
        );
        let expected_refusal = text.contains("cgroup boxing was NOT ESTABLISHED")
            || text.contains("cgroup boxing could not be established");
        assert!(
            output.status.code() == Some(3)
                && explicit_unavailable
                && expected_refusal
                && observation.is_none(),
            "missing direct-cgroup banner was not the expected host refusal (status={:?}):\n{text}",
            output.status.code()
        );
        eprintln!("skipping direct contained-payload case: explicit cgroupfs-unavailable refusal");
        return;
    }

    assert!(
        output.status.success(),
        "matching contained-payload run failed:\n{text}"
    );
    let (pid, start_time, cgroup, populated) =
        observation.expect("contained payload was never observed");
    assert!(
        populated,
        "matching payload cgroup was not observed populated"
    );
    let start_time = start_time.expect("payload must be live before leader release");
    assert!(
        !exact_process_is_present(pid, start_time),
        "the exact matching contained payload survived cgroup teardown"
    );
    assert!(
        !late_marker.exists(),
        "the contained payload survived long enough to publish its late marker"
    );
    assert!(
        !cgroup.exists(),
        "the step cgroup remained after kill + populated=0 proof: {}",
        cgroup.display()
    );
}

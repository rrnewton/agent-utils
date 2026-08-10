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
//! the instrument (`SAFE_CI_FORCE_SCOPE_ATTEMPT=1`) to answer the capability question on a runner
//! population nobody has measured.

use std::path::PathBuf;
use std::process::Command;

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
        let dir = std::env::temp_dir().join(format!("scdr_scope_{name}_{}", std::process::id()));
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

struct Out {
    code: Option<i32>,
    text: String,
}

/// Run the built binary with a controlled environment.
///
/// `hide_session` removes the two variables that make a systemd `--user` bus reachable, which is
/// how a runner with no user manager looks from inside the process.
fn run(fx: &Fixture, envs: &[(&str, &str)], extra: &[&str], hide_session: bool) -> Out {
    let bin = env!("CARGO_BIN_EXE_safe-ci-dag-runner");
    let mut cmd = Command::new(bin);
    cmd.args(["run", "--dag", fx.dag().to_str().unwrap(), "--no-profile"])
        .args(extra)
        // Keep the evidence writer out of this test's way; it is covered elsewhere.
        .env("SAFE_CI_DAG_RUNNER_NO_STEP_LOGS", "1");
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
        r.text.contains("SAFE_CI_FORCE_SCOPE_ATTEMPT"),
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
        &[("CI", "1"), ("SAFE_CI_FORCE_SCOPE_ATTEMPT", "1")],
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
/// This is the path every hermit CI lane actually takes, and it short-circuits ahead of the scope
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
            &format!("--unit=scdr-attempt-probe-{}", std::process::id()),
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
        &[("CI", "1"), ("SAFE_CI_FORCE_SCOPE_ATTEMPT", "1")],
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

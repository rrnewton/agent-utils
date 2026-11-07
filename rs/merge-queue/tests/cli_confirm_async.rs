//! Same-commit confirmation with the probe as an ordinary window flight
//! (#1006 merge-queue-confirm-probe-blocks-run): while a red batch's probe
//! donor job is out, the run keeps taking intake and deciding other batches,
//! and the probe's verdict still decides the red.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const IDENTITY: [(&str, &str); 7] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("TESTTMP", "1"),
];

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_merge-queue")
}

fn command(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(bin());
    command.args(args).current_dir(dir);
    for (key, value) in IDENTITY.iter().chain(env) {
        command.env(key, value);
    }
    command.env("MERGE_QUEUE_ALLOW_DIRTY", "1");
    command.env_remove("MERGE_QUEUE_STATE");
    command
}

fn donor_clone(fx: &Fixture, name: &str) -> PathBuf {
    git(&fx.root, &["clone", "-q", "remote.git", name]);
    fx.root.join(name)
}

fn donor(fx: &Fixture, clone: &Path, name: &str) -> Output {
    let route = format!("{} --repo {}", bin(), fx.queue().display());
    let clone_text = clone.display().to_string();
    command(
        &fx.root,
        &[
            "donor",
            "run",
            "--queue",
            &route,
            "--clone",
            &clone_text,
            "--name",
            name,
            "--poll-secs",
            "1",
            "--until-idle",
        ],
        &[],
    )
    .output()
    .unwrap()
}

fn spawn_run(fx: &Fixture, extra: &[&str]) -> Child {
    let mut args = vec!["run", "--json"];
    args.extend_from_slice(extra);
    command(&fx.queue(), &args, &[])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn finish(run: Child) -> (Option<i32>, Value, String) {
    let out = run.wait_with_output().unwrap();
    let report = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (
        out.status.code(),
        report,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn jobs(fx: &Fixture) -> Vec<Value> {
    let out = fx.mq_ok(&["donor", "status", "--json"]);
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn item_status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

fn set_claim_timeout(fx: &Fixture, secs: u64) {
    let path = fx
        .queue()
        .join(".git")
        .join("merge-queue")
        .join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["donors"] = serde_json::json!({ "claim_timeout_secs": secs });
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

/// A validator gated on files under `gates`: a focused (probe) run waits for
/// `gates/probe`; a candidate holding only tests/x waits for `gates/slow`.
/// A test file with FAIL fails; FLAKY fails only its first run at a commit
/// (counted in `gates/count-<test>`).
fn validator(gates: &Path) -> String {
    format!(
        r#"gates='{gates}'
if [ "$MQ_SELECTION" = tests ]; then
  i=0; while [ ! -e "$gates/probe" ] && [ $i -lt 2400 ]; do sleep 0.05; i=$((i+1)); done
  [ -e "$gates/probe" ] || exit 1
  set -- $MQ_TESTS
else
  if [ -f tests/x ] && [ ! -f tests/a ]; then
    i=0; while [ ! -e "$gates/slow" ] && [ $i -lt 2400 ]; do sleep 0.05; i=$((i+1)); done
    [ -e "$gates/slow" ] || exit 1
  fi
  set -- $(ls tests)
fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  if [ -f "tests/$t" ] && grep -q FLAKY "tests/$t"; then
    n=$(cat "$gates/count-$t" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$gates/count-$t"
    if [ $n -lt 2 ]; then o=fail; rc=1; fi
  fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        gates = gates.display()
    )
}

fn purposes(report: &Value) -> Vec<String> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["purpose"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_queued_probe_does_not_block_intake_or_other_decisions() {
    let fx = Fixture::new("confirm-async-nonblocking");
    let gates = fx.root.join("gates");
    let fx = setup_at(fx, &gates);
    fx.branch("g", &[("tests/x", "ok\n")]);
    fx.branch("r", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "g"]);
    fx.mq_ok(&["submit", "2", "--head", "r"]);
    let clone1 = donor_clone(&fx, "donor-host-1");
    let clone2 = donor_clone(&fx, "donor-host-2");
    let clone3 = donor_clone(&fx, "donor-host-3");
    let run = spawn_run(&fx, &["--depth", "3"]);
    wait_for("the first donor job", Duration::from_secs(60), || {
        !jobs(&fx).is_empty()
    });
    // Persistent donors, killed below (see two_reds_probe_side_by_side:
    // an `--until-idle` donor can exit before later jobs are published).
    let mut d1 = spawn_donor(&fx, &clone1, "d1");
    let mut d2 = spawn_donor(&fx, &clone2, "d2");
    // Both batch jobs are published; the green one is gated, the red
    // one's verdict is in behind it.
    wait_for("two donor jobs", Duration::from_secs(60), || {
        jobs(&fx).len() >= 2
    });
    // Let the green batch finish: it lands, the red is decided, and its
    // probe is published and leased (gated) by the surviving donor.
    std::fs::write(gates.join("slow"), "").unwrap();
    wait_for("the green batch to land", Duration::from_secs(60), || {
        item_status(&fx, 1) == "landed"
    });
    wait_for("the probe to be leased", Duration::from_secs(60), || {
        jobs(&fx).len() >= 3 && jobs(&fx).iter().any(|job| job["state"] == "leased")
    });
    assert_eq!(item_status(&fx, 2), "queued");
    assert!(
        jobs(&fx).iter().any(|job| job["state"] == "leased"),
        "the probe is still out: {:?}",
        jobs(&fx)
    );
    // A recorded submit is applied at a free slot and its batch starts
    // (a donor job is published for it) while the probe is still out.
    fx.branch("c", &[("tests/c", "ok\n")]);
    assert!(fx
        .mq_ok(&["submit", "3", "--head", "c"])
        .contains("recorded"));
    wait_for("the intake batch job", Duration::from_secs(60), || {
        jobs(&fx).len() >= 4
    });
    let mut d3 = spawn_donor(&fx, &clone3, "d3");
    // Release the probe: the red rerun fails again, so #2 is ejected,
    // and the intake batch lands after it.
    std::fs::write(gates.join("probe"), "").unwrap();
    let (code, report, stderr) = finish(run);
    d1.kill().unwrap();
    let _ = d1.wait();
    d2.kill().unwrap();
    let _ = d2.wait();
    d3.kill().unwrap();
    let _ = d3.wait();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    assert_eq!(item_status(&fx, 2), "ejected");
    assert_eq!(item_status(&fx, 3), "landed");
    let ejected = report["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{report}");
    assert!(
        ejected[0]["reason"]
            .as_str()
            .unwrap()
            .contains("(seen twice)"),
        "{report}"
    );
    assert!(purposes(&report).contains(&"probe".to_owned()), "{report}");
}

#[test]
fn a_green_probe_rerun_lands_the_red_as_flaky() {
    let gates_root = Fixture::new("confirm-async-flaky");
    let gates = gates_root.root.join("gates");
    let fx = setup_at(gates_root, &gates);
    fx.branch("r", &[("tests/a", "FLAKY\n")]);
    fx.mq_ok(&["submit", "1", "--head", "r"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &["--depth", "2"]);
    wait_for("the batch job", Duration::from_secs(60), || {
        !jobs(&fx).is_empty()
    });
    std::fs::write(gates.join("probe"), "").unwrap();
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    assert_eq!(purposes(&report), vec!["batch", "probe"], "{report}");
    let landed = report["landed"].as_array().unwrap();
    assert_eq!(landed.len(), 1, "{report}");
}

#[test]
fn two_reds_probe_side_by_side() {
    let fx = Fixture::new("confirm-async-two-reds");
    let gates = fx.root.join("gates");
    let fx = setup_at(fx, &gates);
    // Two roots, so the second red's candidate sits on the base itself and
    // its search is valid before its turn.
    let path = fx
        .queue()
        .join(".git")
        .join("merge-queue")
        .join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(2);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("r1", &[("tests/a", "FAIL\n")]);
    fx.branch("r2", &[("tests/b", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "r1"]);
    fx.mq_ok(&["submit", "2", "--head", "r2"]);
    let clone1 = donor_clone(&fx, "donor-host-1");
    let clone2 = donor_clone(&fx, "donor-host-2");
    let run = spawn_run(&fx, &["--depth", "4"]);
    wait_for("the first donor job", Duration::from_secs(60), || {
        !jobs(&fx).is_empty()
    });
    // Persistent donors, killed below: an `--until-idle` donor exits on
    // its first empty claim, which can precede the second batch's
    // publication, leaving one donor to serialise behind a gated probe
    // while the other batch starves past its claim timeout.
    let mut d1 = spawn_donor(&fx, &clone1, "d1");
    let mut d2 = spawn_donor(&fx, &clone2, "d2");
    // Two batch jobs and both probe jobs exist, both probes leased,
    // before either verdict is in.
    wait_for("four donor jobs", Duration::from_secs(240), || {
        jobs(&fx).len() >= 4
    });
    wait_for("both probes leased", Duration::from_secs(240), || {
        jobs(&fx)
            .iter()
            .filter(|job| job["state"] == "leased")
            .count()
            >= 2
    });
    std::fs::write(gates.join("probe"), "").unwrap();
    let (code, report, stderr) = finish(run);
    d1.kill().unwrap();
    let _ = d1.wait();
    d2.kill().unwrap();
    let _ = d2.wait();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "ejected", "{report}");
    assert_eq!(item_status(&fx, 2), "ejected", "{report}");
    assert_eq!(
        purposes(&report)
            .iter()
            .filter(|purpose| *purpose == "probe")
            .count(),
        2,
        "{report}"
    );
}

/// A donor that keeps running (no `--until-idle`), killed by the test.
fn spawn_donor(fx: &Fixture, clone: &Path, name: &str) -> Child {
    let route = format!("{} --repo {}", bin(), fx.queue().display());
    let clone_text = clone.display().to_string();
    command(
        &fx.root,
        &[
            "donor",
            "run",
            "--queue",
            &route,
            "--clone",
            &clone_text,
            "--name",
            name,
            "--poll-secs",
            "1",
        ],
        &[],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap()
}

fn cancel_job_file(fx: &Fixture, id: &str) {
    let path = fx
        .queue()
        .join(".git")
        .join("merge-queue")
        .join("donor")
        .join("jobs")
        .join(format!("{id}.json"));
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record["state"] = serde_json::json!("cancelled");
    record["history"] = serde_json::json!(["cancelled: by the operator in the test"]);
    std::fs::write(&path, serde_json::to_string_pretty(&record).unwrap()).unwrap();
}

#[test]
fn a_cancelled_probe_is_handled_without_blaming_the_run() {
    let fx = Fixture::new("confirm-async-cancelled");
    let gates = fx.root.join("gates");
    let fx = setup_at(fx, &gates);
    fx.branch("r", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "r"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &["--depth", "2"]);
    let mut donor = spawn_donor(&fx, &clone, "d1");
    // The probe is published and leased (its validation gated) before the
    // cancellation: two jobs, the second one leased.
    wait_for(
        "the probe job to be leased",
        Duration::from_secs(60),
        || jobs(&fx).len() >= 2 && jobs(&fx).iter().any(|job| job["state"] == "leased"),
    );
    let probe_id = jobs(&fx)
        .into_iter()
        .find(|job| job["state"] == "leased")
        .map(|job| job["id"].as_str().unwrap().to_owned())
        .unwrap();
    donor.kill().unwrap();
    let _ = donor.wait();
    cancel_job_file(&fx, &probe_id);
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    // No verdict from the probe: the red run itself decides, and the
    // ejection says the narrowing was skipped.
    assert_eq!(item_status(&fx, 1), "ejected");
    let ejected = report["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{report}");
    assert!(
        ejected[0]["reason"]
            .as_str()
            .unwrap()
            .contains("culprit narrowing"),
        "{report}"
    );
}

#[test]
fn a_probe_no_donor_takes_fails_the_run_and_ejects_nothing() {
    let fx = Fixture::new("confirm-async-failed-probe");
    let gates = fx.root.join("gates");
    let fx = setup_at(fx, &gates);
    // Short leases and claim timeout: a probe nobody takes fails fast.
    let path = fx
        .queue()
        .join(".git")
        .join("merge-queue")
        .join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["donors"] = serde_json::json!({ "claim_timeout_secs": 8, "lease_secs": 3 });
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("r", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "r"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &["--depth", "2"]);
    let mut donor = spawn_donor(&fx, &clone, "d1");
    // Once the batch job is done, no donor remains for the probe.
    wait_for("the batch job to be done", Duration::from_secs(60), || {
        jobs(&fx).iter().any(|job| job["state"] == "done")
    });
    donor.kill().unwrap();
    let _ = donor.wait();
    let (code, _report, stderr) = finish(run);
    assert_ne!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("no donor took") || stderr.contains("probe"),
        "{stderr}"
    );
    assert_eq!(item_status(&fx, 1), "queued");
}

#[test]
fn a_restart_recovers_a_probe_that_is_still_out() {
    let fx = Fixture::new("confirm-async-restart");
    let gates = fx.root.join("gates");
    let fx = setup_at(fx, &gates);
    fx.branch("r", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "r"]);
    let clone = donor_clone(&fx, "donor-host");
    let mut first = spawn_run(&fx, &["--depth", "2"]);
    let mut donor_child = spawn_donor(&fx, &clone, "d1");
    wait_for(
        "the probe job to be leased",
        Duration::from_secs(60),
        || jobs(&fx).len() >= 2 && jobs(&fx).iter().any(|job| job["state"] == "leased"),
    );
    // Kill the run while the probe is out; the change stays queued.
    first.kill().unwrap();
    let _ = first.wait();
    donor_child.kill().unwrap();
    let _ = donor_child.wait();
    assert_eq!(item_status(&fx, 1), "queued");
    // A fresh run revalidates and re-probes; the verdict still decides it.
    std::fs::write(gates.join("probe"), "").unwrap();
    let second = spawn_run(&fx, &["--depth", "2"]);
    wait_for(
        "a queued job for the second run",
        Duration::from_secs(60),
        || jobs(&fx).iter().any(|job| job["state"] == "queued"),
    );
    let out = donor(&fx, &clone, "d2");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, _report, stderr) = finish(second);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "ejected");
}

fn setup_at(fx: Fixture, gates: &Path) -> Fixture {
    std::fs::create_dir_all(gates).unwrap();
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator(gates),
        "--offload",
        "decide",
        "--confirm-reruns",
        "--trust-base",
        "--max-batch",
        "1",
    ]);
    set_claim_timeout(&fx, 120);
    fx
}

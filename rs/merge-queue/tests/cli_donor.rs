//! Donor validators: a second "host" (its own clone) takes queued validations under a lease,
//! validates the exact candidate, and returns a receipt the queue lands on; wrong receipts,
//! wrong tools, dead donors and compare mode. The route is a local path to the binary.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const IDENTITY: [(&str, &str); 6] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
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
    command.env_remove("MERGE_QUEUE_STATE");
    command
}

fn state_dir(fx: &Fixture) -> PathBuf {
    fx.queue().join(".git").join("merge-queue")
}

/// A donor host: its own clone of the remote.
fn donor_clone(fx: &Fixture, name: &str) -> PathBuf {
    git(&fx.root, &["clone", "-q", "remote.git", name]);
    fx.root.join(name)
}

/// The route a donor uses: this binary against the queue clone.
fn route(fx: &Fixture) -> String {
    format!("{} --repo {}", bin(), fx.queue().display())
}

fn donor(fx: &Fixture, clone: &Path, name: &str, env: &[(&str, &str)]) -> Output {
    let route = route(fx);
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
        env,
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
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap()
}

fn set_config(fx: &Fixture, edit: impl FnOnce(&mut Value)) {
    let path = state_dir(fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut config);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

/// Two changes in one batch, so the candidate is a commit only the queue host has.
fn two_changes(fx: &Fixture) {
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.branch("b", &[("b.txt", "b\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
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

#[test]
fn decide_mode_lands_a_candidate_on_a_donor_receipt_alone() {
    let fx = Fixture::new("donor-decide");
    let marker = fx.root.join("validated-in");
    let run_cmd = format!("pwd >> {}; exit 0", marker.display());
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &run_cmd,
        "--offload",
        "decide",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    assert_eq!(item_status(&fx, 1), "landed");
    // The validation ran in the donor's own checkout, never in the queue's.
    let places = std::fs::read_to_string(&marker).unwrap();
    assert!(
        places
            .lines()
            .all(|line| line.contains("merge-queue-donor")),
        "{places}"
    );
    // The candidate came over as a bundle: it is a commit only the queue host had.
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["receipt"]["donor"]["name"], "d1", "{job}");
    assert_eq!(job["receipt"]["commit"], fx.remote_main(), "{job}");
    // Job files are the owner's alone, and no token is ever shown.
    let dir = state_dir(&fx).join("donor").join("jobs");
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        use std::os::unix::fs::PermissionsExt;
        let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", entry.path().display());
    }
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(status.contains("1 done"), "{status}");
    assert!(!fx.mq_ok(&["donor", "status", "--json"]).contains("token"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("token"));
}

#[test]
fn a_dead_donor_loses_its_lease_and_its_late_receipt_is_refused() {
    let fx = Fixture::new("donor-dead");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"lease_secs": 2})
    });
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // A donor claims the job and dies: it never renews or reports.
    let claimed: Value = serde_json::from_str(&fx.mq_ok(&[
        "donor", "claim", "--donor", "ghost", "--host", "gone", "--jobs", "1",
    ]))
    .unwrap();
    let id = claimed["job"]["id"].as_str().unwrap().to_owned();
    let token = claimed["job"]["lease"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    // While the ghost holds it, nobody else can take it.
    let other: Value =
        serde_json::from_str(&fx.mq_ok(&["donor", "claim", "--donor", "d1", "--jobs", "1"]))
            .unwrap();
    assert!(other["job"].is_null(), "{other}");
    std::thread::sleep(Duration::from_secs(3));
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = jobs(&fx)
        .into_iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap();
    assert_eq!(job["attempts"], 2, "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("lease of ghost on gone expired"),
        "{job}"
    );
    // The ghost's late receipt names the right commit but its lease is gone.
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(
            state_dir(&fx)
                .join("donor")
                .join("jobs")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let late = serde_json::json!({
        "token": token,
        "receipt": record["receipt"].clone(),
    });
    let mut child = command(&fx.queue(), &["donor", "report", "--job", &id], &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(late.to_string().as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already done"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_donor_with_different_tools_declines_and_nothing_is_ejected() {
    let fx = Fixture::new("donor-tools");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--tool-identity",
        "toolchain=cat .tool-version",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4})
    });
    std::fs::write(fx.queue().join(".tool-version"), "1.90\n").unwrap();
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    std::fs::write(clone.join(".tool-version"), "1.91\n").unwrap();
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("DECLINED"), "{stderr}");
    assert!(
        stderr.contains(r#"toolchain: queue "1.90", donor "1.91""#),
        "{stderr}"
    );
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(3));
    assert!(
        report["stopped"]
            .as_str()
            .unwrap()
            .contains("no donor took job"),
        "{report}"
    );
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(item_status(&fx, 1), "queued");
    let job = &jobs(&fx)[0];
    assert_eq!(job["declined_by"], serde_json::json!(["d1"]), "{job}");
}

#[test]
fn a_bundle_over_the_limit_is_refused_and_the_donor_declines() {
    let fx = Fixture::new("donor-bundle-limit");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    set_config(
        &fx,
        |config| {
            config["donors"] = serde_json::json!({"max_bundle_bytes": 10, "claim_timeout_secs": 4})
        },
    );
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("over donors.max_bundle_bytes (10)"),
        "{stderr}"
    );
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(3));
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
}

#[test]
fn compare_mode_lands_on_the_local_verdict_and_records_disagreement() {
    // A donor whose environment differs (here it fails where the queue host passes) must
    // never decide: the local verdict lands, and the disagreement is recorded.
    let fx = Fixture::new("donor-compare");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"[ -z "$DONOR_ENV_DIFFERS" ]"#,
        "--offload",
        "compare",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let out = command(&fx.queue(), &["run", "--json"], &[])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(item_status(&fx, 1), "landed");
    let out = donor(&fx, &clone, "d1", &[("DONOR_ENV_DIFFERS", "1")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["local"]["green"], true, "{job}");
    assert_eq!(
        job["comparison"].as_str().unwrap(),
        "mismatch: local green, donor red",
        "{job}"
    );
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(
        status.contains("compare: 0 of 1 receipts match"),
        "{status}"
    );
    // The same environment agrees.
    fx.branch("c", &[("c.txt", "c\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.mq_ok(&["run", "--json"]);
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(
        status.contains("compare: 1 of 2 receipts match"),
        "{status}"
    );
}

#[test]
fn offload_off_publishes_nothing() {
    let fx = Fixture::new("donor-off");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    two_changes(&fx);
    fx.mq_ok(&["run", "--json"]);
    assert_eq!(item_status(&fx, 1), "landed");
    assert!(jobs(&fx).is_empty());
    assert!(!state_dir(&fx).join("donor").join("jobs").exists());
    // The config a queue without donors writes is unchanged: no new keys.
    let config = std::fs::read_to_string(state_dir(&fx).join("config.json")).unwrap();
    assert!(
        !config.contains("offload") && !config.contains("donors"),
        "{config}"
    );
}

#[test]
fn a_stop_while_waiting_for_a_donor_cancels_the_job_without_a_verdict() {
    let fx = Fixture::new("donor-stop");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    two_changes(&fx);
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let sent = Command::new("kill")
        .args(["-TERM", &run.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(3));
    assert!(
        report["stopped"].as_str().unwrap().contains("stop request"),
        "{report}"
    );
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(jobs(&fx)[0]["state"], "cancelled");
    assert_eq!(item_status(&fx, 1), "queued");
}

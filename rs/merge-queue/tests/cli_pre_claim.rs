//! A donor's pre-claim check (`validator.pre_claim_check`): a configured
//! per-candidate gate a donor runs before fetching a job. A failing or
//! timed-out check gives the job back (no decline, no receipt) with the
//! check's first stderr line as the reason.

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

fn spawn_run(fx: &Fixture) -> Child {
    command(&fx.queue(), &["run", "--json"], &[])
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

/// A decided queue with one change, its validator configured by `edit`
/// (called with the fixture root, so checks can name fixture files).
fn setup(tag: &str, edit: impl FnOnce(&Path, &mut Value)) -> Fixture {
    let fx = Fixture::new(tag);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    let path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&fx.root, &mut config);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx
}

#[test]
fn a_passing_pre_claim_check_claims_and_lands_with_the_job_env() {
    let fx = setup("pre-claim-pass", |root, config| {
        let env_file = root.join("pre-claim-env");
        config["validator"]["pre_claim_check"] = serde_json::json!(format!(
            "printf '%s|%s|%s' \"$MQ_CANDIDATE\" \"$MQ_REPO\" \"$MQ_BASE\" > '{}'; exit 0",
            env_file.display()
        ));
    });
    let env_file = fx.root.join("pre-claim-env");
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let job = jobs(&fx)[0].clone();
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
    let env = std::fs::read_to_string(&env_file).unwrap();
    let parts: Vec<&str> = env.split('|').collect();
    assert_eq!(parts.len(), 3, "{env}");
    assert_eq!(parts[0], job["commit"].as_str().unwrap(), "{env}");
    assert!(parts[1].contains("remote.git"), "{env}");
    assert_eq!(parts[2], job["base"].as_str().unwrap(), "{env}");
}

#[test]
fn a_failing_pre_claim_check_releases_without_declining_and_the_job_is_offered_again() {
    let fx = setup("pre-claim-fail", |root, config| {
        config["validator"]["pre_claim_check"] = serde_json::json!(format!(
            "[ -e '{}/allow' ] || {{ echo 'pinned image not present' >&2; exit 1; }}",
            root.display()
        ));
    });
    let allow = fx.root.join("allow");
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let job = jobs(&fx)[0].clone();
    assert_eq!(job["state"], "queued", "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("pinned image not present"),
        "{job}"
    );
    assert!(
        job["declined_by"].as_array().is_none_or(Vec::is_empty),
        "{job}"
    );
    assert!(
        job["could_not_run"].as_array().is_none_or(Vec::is_empty),
        "{job}"
    );
    assert!(job["receipt"].is_null(), "{job}");
    // Offered again: once the host has what the candidate needs, the
    // same donor takes the job and the change lands in the same run.
    std::fs::write(&allow, "").unwrap();
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
}

#[test]
fn a_timed_out_pre_claim_check_releases_with_the_timeout_reason() {
    let fx = setup("pre-claim-timeout", |root, config| {
        config["validator"]["pre_claim_check"] =
            serde_json::json!(format!("[ -e '{}/allow' ] || sleep 10", root.display()));
        config["validator"]["pre_claim_timeout_secs"] = serde_json::json!(1);
    });
    let allow = fx.root.join("allow");
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let started = Instant::now();
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "the donor waited out the check's sleep: {:?}",
        started.elapsed()
    );
    let job = jobs(&fx)[0].clone();
    assert_eq!(job["state"], "queued", "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("pre_claim_check timed out"),
        "{job}"
    );
    std::fs::write(&allow, "").unwrap();
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
}

#[test]
fn no_pre_claim_check_claims_as_before() {
    let fx = setup("pre-claim-absent", |_, _| {});
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
}

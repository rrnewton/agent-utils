//! A donor's early verdict reaches the queue: with `validator.early_verdict`
//! on, a donor runner that declares `FAILED <test>` in its early-verdict
//! file is stopped at once, and the queue records the batch validation
//! with `early_red: true` instead of a plain red.

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

#[test]
fn a_donor_early_verdict_is_recorded_as_an_early_red() {
    let fx = Fixture::new("donor-early-verdict");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // The runner declares its red final in the early-verdict file, then
    // sleeps: only the early-verdict watch can decide it before that ends.
    let validator = r#"if [ -f tests/a ]; then printf 'FAILED a\n' > "$MQ_EARLY_VERDICT"; sleep 30; exit 1; fi; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
        "--offload",
        "decide",
    ]);
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["validator"]["early_verdict"] = serde_json::json!(true);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host");
    let started = Instant::now();
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
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the queue waited for a sleeping donor runner: {:?}",
        started.elapsed()
    );
    let early_batches: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["early_red"] == true)
        .collect();
    assert_eq!(early_batches.len(), 1, "{report}");
    assert_eq!(item_status(&fx, 1), "ejected");
    let job = &jobs(&fx)[0];
    assert_eq!(job["receipt"]["early_red"], true, "{job}");
}

//! Front starvation (#1196 front-starve): a donor must claim in plan order
//! (a speculative prefix flight claims under its batch's index), and an
//! unstarted front (its donor job still queued, no validation spent) must
//! not block a green disjoint root behind it.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

fn state_dir(fx: &Fixture) -> PathBuf {
    fx.queue().join(".git").join("merge-queue")
}

fn donor_clone(fx: &Fixture, name: &str) -> PathBuf {
    git(&fx.root, &["clone", "-q", "remote.git", name]);
    fx.root.join(name)
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

fn spawn_donor(fx: &Fixture, clone: &Path, name: &str, env: &[(&str, &str)]) -> Child {
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
            "--mismatch-backoff-secs",
            "0",
        ],
        env,
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
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
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

fn set_config(fx: &Fixture, edit: impl FnOnce(&mut Value)) {
    let path = state_dir(fx).join("config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut config);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
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
fn an_unstarted_front_does_not_block_a_green_disjoint_root() {
    let fx = Fixture::new("front-starve");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let gate = fx.root.join("front-gate");
    // On the donor, the front's own candidate (its tip commit changes only
    // tests/a) cannot run until the gate file exists; everything else,
    // stacked candidates included, passes. The front's donor claims
    // therefore give the job back unvalidated, and the front flight stays
    // unstarted while the disjoint roots validate green.
    let validator = format!(
        r#"if [ -n "$MQ_PROBE" ]; then exit 0; fi; if [ -n "$DONOR" ] && [ "$(git diff --name-only HEAD^ HEAD)" = "tests/a" ] && [ ! -e {} ]; then echo COULD_NOT_RUN; exit 75; fi; for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#,
        gate.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--offload",
        "decide",
        "--max-batch",
        "1",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 60});
        config["speculative_stacking"] = serde_json::json!(false)
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &["--depth", "3"]);
    wait_for("published jobs", Duration::from_secs(60), || {
        !jobs(&fx).is_empty()
    });
    let mut donor = spawn_donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    // A disjoint root lands while the front was never validated.
    let root_landed = (0..1200).any(|_| {
        if item_status(&fx, 2) == "landed" || item_status(&fx, 3) == "landed" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
        false
    });
    if !root_landed {
        panic!(
            "no root landed; statuses 1={} 2={} 3={}; jobs={}",
            item_status(&fx, 1),
            item_status(&fx, 2),
            item_status(&fx, 3),
            serde_json::to_string(&jobs(&fx)).unwrap()
        );
    }
    assert_eq!(item_status(&fx, 1), "queued");
    // Let the front run: a second run rebuilds it on the new base and
    // lands it (the first run deferred it after the refusal bound).
    std::fs::write(&gate, "").unwrap();
    let (_code, report, _stderr) = finish(run);
    // The first run deferred the front with a refusal backoff; let it lapse.
    std::thread::sleep(Duration::from_secs(10));
    let run2 = spawn_run(&fx, &["--depth", "3"]);
    let (code2, report2, stderr2) = finish(run2);
    donor.kill().unwrap();
    let _ = donor.wait();
    assert_eq!(code2, Some(0), "{stderr2}");
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(item_status(&fx, 1), "landed");
    let mut combined = report.clone();
    combined["validations"] = Value::Array(
        report["validations"]
            .as_array()
            .unwrap()
            .iter()
            .chain(report2["validations"].as_array().unwrap().iter())
            .cloned()
            .collect(),
    );
    let report = &combined;
    let landed: Vec<Vec<i64>> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .chain(report2["landed"].as_array().unwrap().iter())
        .map(|batch| {
            batch["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect()
        })
        .collect();
    let mut all: Vec<i64> = landed.iter().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, vec![1, 2, 3], "{report}");
    assert_eq!(
        landed[0],
        vec![3],
        "the disjoint root landed before the front: {report}"
    );
    // No green was thrown away: the root that landed first validated as a
    // batch exactly once, green.
    let root_runs: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == Value::from(vec![3]))
        .collect();
    assert_eq!(root_runs.len(), 1, "root #3 validated once: {report}");
    assert_eq!(root_runs[0]["green"], true, "{report}");
}

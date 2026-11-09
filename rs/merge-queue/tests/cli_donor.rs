//! Donor validators: a second "host" (its own clone) takes queued validations under a lease,
//! validates the exact candidate, and returns a receipt the queue lands on; wrong receipts,
//! wrong tools, dead donors and compare mode. The route is a local path to the binary.

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
    // Marks a test run (see support/mod.rs).
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
    // Tests build from a working tree that may hold uncommitted changes.
    command.env("MERGE_QUEUE_ALLOW_DIRTY", "1");
    command.env_remove("MERGE_QUEUE_STATE");
    command
}

/// The revision `merge-queue --version` reports.
fn own_revision() -> String {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("commit "))
        .unwrap_or_else(|| panic!("no commit line in {text:?}"));
    let commit = line.split(' ').next().unwrap().to_owned();
    assert!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "a test build comes from a checkout and records its revision: {text:?}"
    );
    commit
}

/// Write a donor job as a donor or the sweep would: atomically, so a
/// concurrent reader in the run never sees a truncated file.
fn write_job(path: &Path, job: &Value) {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(job).unwrap()).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

fn job_path(fx: &Fixture, id: &str) -> PathBuf {
    state_dir(fx)
        .join("donor")
        .join("jobs")
        .join(format!("{id}.json"))
}

/// Run `merge-queue donor VERB --job ID` in the queue with `stdin`; (success, stdout, stderr).
fn donor_call(fx: &Fixture, verb: &str, id: &str, stdin: &str) -> (bool, String, String) {
    let mut child = command(&fx.queue(), &["donor", verb, "--job", id], &[])
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
            .write_all(stdin.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
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

/// Sets a donor-loop done flag when dropped, so a panic inside a donor
/// thread scope cannot deadlock the scope join on donor threads that run
/// until the flag is set.
struct DoneOnDrop<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for DoneOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
    // Both sides name the merge-queue revision they run, the one `--version` prints.
    let revision = own_revision();
    assert_eq!(
        job["tool"]["merge_queue"]["commit"],
        revision.as_str(),
        "{job}"
    );
    assert_eq!(job["receipt"]["tool"], job["tool"], "{job}");
    // Job files are the owner's alone, and no token is ever shown.
    let dir = state_dir(&fx).join("donor").join("jobs");
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        use std::os::unix::fs::PermissionsExt;
        let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", entry.path().display());
    }
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(status.contains("1 done"), "{status}");
    assert!(status.contains("merge-queue here: "), "{status}");
    assert!(!fx.mq_ok(&["donor", "status", "--json"]).contains("token"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("token"));
    // The decision log names the donor that validated the landed batch.
    let records = fx.mq_ok(&["stats", "--records"]);
    let decision: serde_json::Value =
        serde_json::from_str(records.lines().next().unwrap()).unwrap();
    assert_eq!(decision["validator"]["kind"], "donor", "{decision}");
    assert!(
        decision["validator"]["host"]
            .as_str()
            .is_some_and(|host| host.starts_with("d1@")),
        "{decision}"
    );
    assert_eq!(
        decision["landed"]["prs"],
        serde_json::json!([1, 2]),
        "{decision}"
    );
}

#[test]
fn a_donor_validator_sees_the_job_base_and_candidate_parent() {
    let fx = Fixture::new("donor-base-parent");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let base_start = git(&fx.dev(), &["rev-parse", "main"]);
    let base_start = base_start.trim().to_owned();
    let marker = fx.root.join("donor-saw-env");
    let run_cmd = format!(
        "echo \"base=$MQ_BASE parent=$MQ_PARENT\" >> {}; exit 0",
        marker.display()
    );
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &run_cmd,
        "--offload",
        "decide",
        "--max-batch",
        "1",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let done = std::sync::atomic::AtomicBool::new(false);
    let (code, _report, stderr) = std::thread::scope(|scope| {
        let (fx2, done2) = (&fx, &done);
        let clone2 = clone.clone();
        scope.spawn(move || {
            while !done2.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = donor(fx2, &clone2, "d1", &[]);
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let outcome = finish(run);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    assert_eq!(item_status(&fx, 2), "landed");
    let all = jobs(&fx);
    let first = all
        .iter()
        .find(|job| job["parent"] == base_start.as_str())
        .expect("a root job recorded with the base as its parent");
    assert_eq!(first["base"], base_start.as_str(), "{first}");
    let first_commit = first["commit"].as_str().unwrap();
    let stacked = all
        .iter()
        .find(|job| job["parent"] == first_commit)
        .expect("a stacked job recorded on the first candidate");
    assert_eq!(stacked["base"], base_start.as_str(), "{stacked}");
    let seen = std::fs::read_to_string(&marker).unwrap();
    assert!(
        seen.lines()
            .any(|line| line == format!("base={base_start} parent={base_start}")),
        "root donor run did not see base as its parent: {seen}"
    );
    assert!(
        seen.lines()
            .any(|line| line == format!("base={base_start} parent={first_commit}")),
        "stacked donor run did not see the first candidate as its parent: {seen}"
    );
}

/// A donor job for a root planned after main moved mid-run records
/// the root's real base (#1111): base == parent == the new base,
/// not the base the run started on, and the donor validator is
/// handed that base.
#[test]
fn a_mid_run_root_donor_job_records_the_base_it_was_planned_on() {
    let fx = Fixture::new("donor-mid-run-root-base");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let base_start = fx.remote_main();
    let marker = fx.root.join("donor-saw-env");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    let run_cmd = format!(
        "set=$(ls tests | tr '\\n' _); echo \"base=$MQ_BASE parent=$MQ_PARENT\" >> {}; i=0; while [ ! -e '{}/'$set ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; exit 0",
        marker.display(),
        gates.display()
    );
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &run_cmd,
        "--offload",
        "decide",
        "--max-batch",
        "1",
    ]);
    set_config(&fx, |config| {
        config["roots"] = serde_json::json!(3);
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    // Change 1 may validate at once; change 2 stays gated in flight.
    std::fs::write(gates.join("a_t0_"), "").unwrap();
    let clone = donor_clone(&fx, "donor-host");
    let route_text = route(&fx);
    let clone_text = clone.display().to_string();
    let donor_daemon = command(
        &fx.root,
        &[
            "donor",
            "run",
            "--queue",
            &route_text,
            "--clone",
            &clone_text,
            "--name",
            "d1",
            "--jobs",
            "2",
            "--poll-secs",
            "1",
        ],
        &[],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    struct KillOnDrop(Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _donor_daemon = KillOnDrop(donor_daemon);
    let run = spawn_run(&fx, &["--depth", "3"]);
    wait_for(
        "change 1 to land on a donor receipt",
        Duration::from_secs(60),
        || item_status(&fx, 1) == "landed",
    );
    let new_base = fx.remote_main();
    assert_ne!(new_base, base_start);
    // Change 2's stale root is rebuilt on the new base and claimed
    // again (its validation writes the third marker line); only then
    // is every later job on the new base change 3's.
    wait_for(
        "change 2 to rebuild on the new base",
        Duration::from_secs(60),
        || {
            std::fs::read_to_string(&marker)
                .map(|seen| seen.lines().count() >= 3)
                .unwrap_or(false)
        },
    );
    // A new root planned now stands on the moved base; its donor job
    // must record that base, not where the run started. (Change 2's
    // rebuilt root also stands on the new base, so name only jobs
    // published from here on.)
    let known: Vec<String> = jobs(&fx)
        .iter()
        .map(|job| job["id"].as_str().unwrap().to_owned())
        .collect();
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    wait_for(
        "the mid-run root job to be published",
        Duration::from_secs(60),
        || {
            jobs(&fx).iter().any(|job| {
                job["parent"] == new_base.as_str()
                    && !known.iter().any(|id| job["id"] == id.as_str())
            })
        },
    );
    let mid = jobs(&fx)
        .into_iter()
        .find(|job| {
            job["parent"] == new_base.as_str() && !known.iter().any(|id| job["id"] == id.as_str())
        })
        .expect("a donor job recorded on the new base");
    assert_eq!(mid["base"], new_base.as_str(), "{mid}");
    for set in [
        "a_t0_",
        "b_t0_",
        "c_t0_",
        "a_b_t0_",
        "a_c_t0_",
        "b_c_t0_",
        "a_b_c_t0_",
        "t0_",
    ] {
        std::fs::write(gates.join(set), "").unwrap();
    }
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    for pr in 1..=3 {
        assert_eq!(
            item_status(&fx, pr),
            "landed",
            "#{pr}: {report} {stderr} jobs={:?}",
            jobs(&fx)
        );
    }
    let seen = std::fs::read_to_string(&marker).unwrap();
    assert!(
        seen.lines()
            .any(|line| line == format!("base={new_base} parent={new_base}")),
        "mid-run root donor run did not see the new base: {seen}"
    );
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
    // The ghost's grant has expired (its expiry is stored, so this cannot
    // revive it); give the live donor a lease it cannot outrun mid-work, so
    // `attempts` counts claims, not scheduling luck under load.
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"lease_secs": 600})
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
fn a_donor_with_different_tools_and_no_refresh_gives_the_job_back() {
    // With no `tool_refresh` the donor cannot tell whether it is older or newer than the job,
    // so it gives the job back (to be offered it again) instead of declining it forever.
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
    assert!(stderr.contains("gave back"), "{stderr}");
    assert!(!stderr.contains("DECLINED"), "{stderr}");
    assert!(
        stderr.contains(r#"toolchain: queue "1.90", donor "1.91""#)
            && stderr.contains("validator.tool_refresh has no command for \"toolchain\""),
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
    assert!(job.get("declined_by").is_none(), "{job}");
    assert!(
        job["history"].to_string().contains("released by d1"),
        "{job}"
    );
}

#[test]
fn an_older_donor_refreshes_its_tools_and_takes_the_job() {
    // The validator's tooling moved and the queue host has the new one; the donor's copy is
    // older. It runs `tool_refresh` (a fetch, in real use), matches, and validates.
    let fx = Fixture::new("donor-refresh-older");
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
        "--tool-refresh",
        "toolchain=echo 1.91 > .tool-version",
    ]);
    std::fs::write(fx.queue().join(".tool-version"), "1.91\n").unwrap();
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    std::fs::write(clone.join(".tool-version"), "1.90\n").unwrap();
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("refreshed toolchain for")
            && stderr.contains(r#"toolchain: queue "1.91", donor "1.90""#),
        "{stderr}"
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(
        job["receipt"]["tool"]["tools"]["toolchain"], "1.91",
        "{job}"
    );
    assert!(job.get("declined_by").is_none(), "{job}");
}

#[test]
fn a_donor_still_different_after_refreshing_its_tools_gives_the_job_back() {
    // After the refresh the donor has the newest tooling, and the job still names another:
    // the donor is newer than the job (or unrelated to it). It gives the job back rather
    // than decline it (#1005): no `declined_by`, so once the queue re-stamps the job the
    // donor is offered it again, and another donor may take it meanwhile.
    let fx = Fixture::new("donor-refresh-newer");
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
        "--tool-refresh",
        "toolchain=echo 1.91 > .tool-version",
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
    // Keep the queue host from refreshing its own stale stamp at claim time, so the
    // donor still meets the mismatch this test is about.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        state_dir(&fx).join("donor").join("tool-refresh-at"),
        now.to_string(),
    )
    .unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gave back"), "{stderr}");
    assert!(!stderr.contains("DECLINED"), "{stderr}");
    assert!(
        stderr.contains("still differ from the job's after refreshing them")
            && stderr.contains("newer than the job or unrelated to it"),
        "{stderr}"
    );
    let job = &jobs(&fx)[0];
    assert!(job.get("declined_by").is_none(), "{job}");
    assert!(
        job["history"].to_string().contains("released by d1"),
        "{job}"
    );
    // The queue host's tools move to the donor's: the next claim re-stamps the job and
    // the same donor takes it.
    std::fs::write(fx.queue().join(".tool-version"), "1.91\n").unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(0));
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert!(job.get("declined_by").is_none(), "{job}");
}

#[test]
fn the_queue_host_refreshes_a_stale_stamp_before_claim() {
    // The job was stamped with the queue host's stale tools. At claim time the queue
    // host runs `tool_refresh` itself (bounded, rate-limited), so the claim re-stamps
    // the job with the fresh answer and the donor whose tools already moved takes it.
    // Without the queue-side refresh the donor still differs after its own refresh
    // and gives the job back (#1005).
    let fx = Fixture::new("queue-refresh-stale");
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
        "--tool-refresh",
        "toolchain=echo 1.91 > .tool-version",
    ]);
    std::fs::write(fx.queue().join(".tool-version"), "1.90\n").unwrap();
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    std::fs::write(clone.join(".tool-version"), "1.91\n").unwrap();
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    assert_eq!(
        jobs(&fx)[0]["tool"]["tools"]["toolchain"],
        "1.90",
        "the job starts with the stale stamp"
    );
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(fx.queue().join(".tool-version"))
            .unwrap()
            .trim(),
        "1.91",
        "the queue host refreshed its own tools at claim time"
    );
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(0));
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(
        job["receipt"]["tool"]["tools"]["toolchain"], "1.91",
        "{job}"
    );
    assert!(job.get("declined_by").is_none(), "{job}");
}

/// A tool repository beside the queue and donor clones: `tool-remote.git`
/// seeded with `ci-hub/tool` v1, cloned as `tool/` inside the queue repo and
/// inside `clone`. Returns the v1 `ci-hub` tree.
fn tool_clones(fx: &Fixture, clone: &Path) -> String {
    git(&fx.root, &["init", "-q", "--bare", "tool-remote.git"]);
    git(&fx.root, &["clone", "-q", "tool-remote.git", "tool-seed"]);
    let seed = fx.root.join("tool-seed");
    std::fs::create_dir_all(seed.join("ci-hub")).unwrap();
    std::fs::write(seed.join("ci-hub/tool"), "v1\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "tool v1"]);
    git(&seed, &["push", "-q", "origin", "HEAD:main"]);
    git(
        &fx.root,
        &["clone", "-q", "tool-remote.git", "queue-tool-tmp"],
    );
    std::fs::rename(fx.root.join("queue-tool-tmp"), fx.queue().join("tool")).unwrap();
    git(
        &fx.root,
        &["clone", "-q", "tool-remote.git", "donor-tool-tmp"],
    );
    std::fs::rename(fx.root.join("donor-tool-tmp"), clone.join("tool")).unwrap();
    git(
        &fx.queue().join("tool"),
        &["rev-parse", "origin/main:ci-hub"],
    )
}

/// Advance the tool repository to v2 (only `ci-hub/tool` changes).
fn advance_tool(fx: &Fixture) {
    let seed = fx.root.join("tool-seed");
    std::fs::write(seed.join("ci-hub/tool"), "v2\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "tool v2"]);
    git(&seed, &["push", "-q", "origin", "HEAD:main"]);
}

#[test]
fn a_remote_donor_with_newer_descendant_tools_gets_the_job() {
    // #1005 acceptance: the job is stamped with the queue host's tool tree;
    // the remote donor refreshes to a descendant tree and runs the job under
    // its own tools, even though the stamp never moved. The receipt records
    // the donor's tree, and the queue accepts it after the same proof.
    let fx = Fixture::new("donor-descendant");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--tool-identity",
        "ci-hub=git -C tool rev-parse origin/main:ci-hub",
        "--tool-refresh",
        "ci-hub=git -C tool fetch -q origin",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let v1_tree = tool_clones(&fx, &clone);
    // The validator attests, from inside the run, the tree in the donor's
    // tool clone it loaded (#1045): after the donor's refresh that is v2.
    let attest = format!(
        "echo merge-queue-tool-identity: ci-hub $(git -C {} rev-parse origin/main:ci-hub); exit 0",
        clone.join("tool").display()
    );
    set_config(&fx, |config| {
        config["validator"]["run"] = serde_json::json!(attest);
    });
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    assert_eq!(
        jobs(&fx)[0]["tool"]["tools"]["ci-hub"],
        v1_tree,
        "the job starts stamped with the queue host's tree"
    );
    // Keep the queue host from re-stamping at claim time, so the donor itself
    // meets the descendant mismatch this test is about.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        state_dir(&fx).join("donor").join("tool-refresh-at"),
        now.to_string(),
    )
    .unwrap();
    advance_tool(&fx);
    // The donor host has already fetched the newer tools (its refresh at claim
    // time will fetch again and stay there); the queue host has not.
    git(&clone.join("tool"), &["fetch", "-q", "origin"]);
    let v2_tree = git(&fx.root.join("tool-seed"), &["rev-parse", "HEAD:ci-hub"]);
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("descend from"), "{stderr}");
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(0));
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["tool"]["tools"]["ci-hub"], v1_tree, "{job}");
    assert_eq!(job["receipt"]["tool"]["tools"]["ci-hub"], v2_tree, "{job}");
    assert_eq!(
        job["receipt"]["ran_tools"]["ci-hub"],
        serde_json::json!([v2_tree]),
        "{job}"
    );
    assert!(job.get("declined_by").is_none(), "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("accepted with descendant tool ci-hub"),
        "{job}"
    );
}

#[test]
fn a_donor_whose_validator_ran_a_stale_tree_is_refused_as_identity_mismatch() {
    // #1045, the observed failure shape: identity answers read the fresh
    // origin/main tree on both hosts, but the validator on the donor loads
    // the stale v1 tree and says so from inside the run. Pre-fix this receipt
    // landed the batch; now the queue refuses it and nothing lands on it.
    let fx = Fixture::new("donor-ran-stale");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--tool-identity",
        "ci-hub=git -C tool rev-parse origin/main:ci-hub",
        "--tool-refresh",
        "ci-hub=git -C tool fetch -q origin",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4, "lease_secs": 30})
    });
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let v1_tree = tool_clones(&fx, &clone);
    advance_tool(&fx);
    // Both hosts' identity answers are fresh (v2)...
    git(&fx.queue().join("tool"), &["fetch", "-q", "origin"]);
    git(&clone.join("tool"), &["fetch", "-q", "origin"]);
    // ...but the validator attests the stale tree it actually loaded.
    let attest_stale = format!("echo merge-queue-tool-identity: ci-hub {v1_tree}; exit 0");
    set_config(&fx, |config| {
        config["validator"]["run"] = serde_json::json!(attest_stale);
    });
    let mut run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    wait_for(
        "the identity-mismatch refusal",
        Duration::from_secs(30),
        || {
            jobs(&fx)[0]["history"]
                .to_string()
                .contains("identity-mismatch")
        },
    );
    let job = &jobs(&fx)[0];
    assert_ne!(job["state"], "done", "{job}\n{stderr}");
    assert!(job.get("receipt").is_none_or(|r| r.is_null()), "{job}");
    run.kill().unwrap();
    run.wait().unwrap();
}

#[test]
fn a_donor_without_identity_lines_is_accepted_unattested_by_default() {
    // #1049: the validator prints no merge-queue-tool-identity line at all
    // (a consumer that has not taught it to yet). With
    // require_tool_identity_lines off (the default) the receipt still
    // lands, recorded as unattested on the job and the receipt.
    let fx = Fixture::new("donor-unattested");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--tool-identity",
        "ci-hub=git -C tool rev-parse origin/main:ci-hub",
        "--tool-refresh",
        "ci-hub=git -C tool fetch -q origin",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let _v1_tree = tool_clones(&fx, &clone);
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
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(0));
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1, 2]),
        "{report}"
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(
        job["tool_identity_unattested"].as_array().map(Vec::len),
        Some(1),
        "{job}"
    );
    assert!(
        job["tool_identity_unattested"][0]
            .as_str()
            .unwrap()
            .contains("tool_identity_unattested"),
        "{job}"
    );
    assert!(
        job["receipt"]["tool_warnings"][0]
            .as_str()
            .unwrap()
            .contains("tool_identity_unattested"),
        "{job}"
    );
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
    // The local validation takes a while, so the donor claims before the candidate lands (a
    // job nobody had claimed by then is cancelled).
    let fx = Fixture::new("donor-compare");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"[ -n "$DONOR" ] || sleep 8; [ -z "$DONOR_ENV_DIFFERS" ]"#,
        "--offload",
        "compare",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(
        &fx,
        &clone,
        "d1",
        &[("DONOR", "1"), ("DONOR_ENV_DIFFERS", "1")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, _, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
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
    let run = spawn_run(&fx, &[]);
    wait_for("a second published job", Duration::from_secs(30), || {
        jobs(&fx).len() == 2
    });
    let out = donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, _, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(
        status.contains("compare: 1 of 2 receipts match"),
        "{status}"
    );
}

#[test]
fn a_donor_built_from_another_revision_gives_the_job_back_and_nothing_is_ejected() {
    // Only an upgrade can match a revision, so the donor gives the job back instead of
    // declining it: once upgraded, it is offered the job again.
    let fx = Fixture::new("donor-revision");
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
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4})
    });
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // The queue host's merge-queue came from another revision.
    let id = jobs(&fx)[0]["id"].as_str().unwrap().to_owned();
    let path = job_path(&fx, &id);
    let mut job: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let elsewhere = "0".repeat(40);
    job["tool"]["merge_queue"] = serde_json::json!({"commit": elsewhere});
    write_job(&path, &job);
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("gave back"), "{stderr}");
    assert!(!stderr.contains("DECLINED"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "this donor's merge-queue is built from {}",
            own_revision()
        )) && stderr.contains(&format!("the queue host's from {elsewhere}")),
        "{stderr}"
    );
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(3));
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    assert!(jobs(&fx)[0].get("declined_by").is_none(), "{:?}", jobs(&fx));
}

#[test]
fn compare_mode_keeps_a_leased_job_after_landing_and_compares_its_late_receipt() {
    let fx = Fixture::new("donor-landed");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "sleep 3",
        "--offload",
        "compare",
    ]);
    two_changes(&fx);
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // A donor holds the lease while the local verdict lands the candidate.
    let claimed: Value = serde_json::from_str(&fx.mq_ok(&[
        "donor", "claim", "--donor", "slow", "--host", "h", "--jobs", "1",
    ]))
    .unwrap();
    let id = claimed["job"]["id"].as_str().unwrap().to_owned();
    let token = claimed["job"]["lease"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    let job = jobs(&fx)
        .into_iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap();
    assert_eq!(job["state"], "leased", "{job}");
    assert_eq!(job["candidate_gone"], "its candidate landed", "{job}");
    // The donor carries on, and its receipt is compared (it cannot affect landing).
    let (ok, out, err) = donor_call(&fx, "renew", &id, &token);
    assert!(ok, "{err}");
    let renewal: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(renewal["renewal"], "continue", "{renewal}");
    let receipt = serde_json::json!({
        "schema": "merge-queue-receipt/v1",
        "job": id,
        "donor": {"name": "slow", "host": "h"},
        "commit": job["commit"],
        "tree": job["tree"],
        "tool": job["tool"],
        "merge_queue_version": "0.1.0",
        "exit": 0,
        "results": {"results": {}},
        "elapsed_ms": 1,
        "started_at": "2026-11-03T00:00:00Z",
        "finished_at": "2026-11-03T00:00:01Z",
    });
    let (ok, _, err) = donor_call(
        &fx,
        "report",
        &id,
        &serde_json::json!({"token": token, "receipt": receipt}).to_string(),
    );
    assert!(ok, "{err}");
    let job = jobs(&fx)
        .into_iter()
        .find(|j| j["id"] == id.as_str())
        .unwrap();
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["comparison"], "match", "{job}");
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(
        status.contains("compared after landing: 1 of 1"),
        "{status}"
    );
}

#[test]
fn compare_mode_publishes_only_the_candidate_and_cancels_its_lease_when_it_is_ejected() {
    // A red batch's culprit search reruns the failed test on the very same candidate (the
    // flaky check) and on the base. None of that is published for donors: the candidate was
    // already published once. And once the candidate is ejected, the donor holding its lease
    // is told to stop instead of finishing a validation nobody will use.
    let fx = Fixture::new("donor-ejected");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"sleep 3; rc=0; for t in $(ls tests 2>/dev/null); do o=pass; if grep -q FAIL "tests/$t"; then o=fail; rc=1; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit $rc"#,
        "--offload",
        "compare",
        "--confirm-reruns",
    ]);
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.branch("b", &[("b.txt", "b\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let claimed: Value = serde_json::from_str(&fx.mq_ok(&[
        "donor", "claim", "--donor", "slow", "--host", "h", "--jobs", "1",
    ]))
    .unwrap();
    let id = claimed["job"]["id"].as_str().unwrap().to_owned();
    let token = claimed["job"]["lease"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "ejected", "{report}");
    assert_eq!(item_status(&fx, 2), "landed", "{report}");
    let probes = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "probe")
        .count();
    assert!(probes > 0, "the search must have probed: {report}");
    // One job for the first candidate and one for the remainder, which is a new
    // candidate once #1 is out; none for any probe.
    let all = jobs(&fx);
    let commits: Vec<&str> = all.iter().map(|j| j["commit"].as_str().unwrap()).collect();
    assert_eq!(all.len(), 2, "{all:?}");
    assert_ne!(commits[0], commits[1], "{all:?}");
    let job = all.iter().find(|j| j["id"] == id.as_str()).unwrap();
    assert_eq!(job["state"], "cancelled", "{job}");
    let (ok, out, err) = donor_call(&fx, "renew", &id, &token);
    assert!(ok, "{err}");
    let renewal: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(renewal["renewal"], "stop", "{renewal}");
}

#[test]
fn a_claim_offers_a_job_under_the_tools_the_queue_host_has_now() {
    // The validator's own tooling moved after the job was published (a fix landed in it).
    // A claim re-stamps the queued job with the queue host's current answer, so donors that
    // already have the new tooling take it instead of declining it forever.
    let fx = Fixture::new("donor-restamp");
    let tool = fx.root.join("tooling-revision");
    std::fs::write(&tool, "old\n").unwrap();
    let identity = format!("tooling=cat {}", tool.display());
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "sleep 3",
        "--offload",
        "compare",
        "--tool-identity",
        &identity,
    ]);
    two_changes(&fx);
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    assert_eq!(jobs(&fx)[0]["tool"]["tools"]["tooling"], "old");
    std::fs::write(&tool, "new\n").unwrap();
    let claimed: Value = serde_json::from_str(&fx.mq_ok(&[
        "donor", "claim", "--donor", "fresh", "--host", "h", "--jobs", "1",
    ]))
    .unwrap();
    assert_eq!(
        claimed["job"]["tool"]["tools"]["tooling"], "new",
        "{claimed}"
    );
    assert!(
        claimed["job"]["history"].to_string().contains(
            "re-stamped with the queue host's current tools (tooling=old -> tooling=new)"
        ),
        "{claimed}"
    );
    let (code, _, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
}

#[test]
fn an_unleased_job_whose_candidate_landed_is_cancelled() {
    let fx = Fixture::new("donor-unleased");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "compare",
    ]);
    two_changes(&fx);
    let (code, _, stderr) = finish(spawn_run(&fx, &[]));
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "cancelled", "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("cancelled: its candidate landed"),
        "{job}"
    );
    // A donor that comes later finds nothing to do.
    let clone = donor_clone(&fx, "donor-host");
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("took"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_job_left_by_a_killed_run_is_cancelled_by_the_next_run() {
    let fx = Fixture::new("donor-sweep");
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
        config["donors"] = serde_json::json!({"claim_timeout_secs": 2})
    });
    two_changes(&fx);
    let mut run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // Killed outright: it settles nothing.
    run.kill().unwrap();
    run.wait().unwrap();
    let stale = jobs(&fx)[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(jobs(&fx)[0]["state"], "queued");
    let (_, _, stderr) = finish(spawn_run(&fx, &[]));
    assert!(
        stderr.contains("cancelled 1 donor job(s) an earlier run left live"),
        "{stderr}"
    );
    let job = jobs(&fx)
        .into_iter()
        .find(|j| j["id"] == stale.as_str())
        .unwrap();
    assert_eq!(job["state"], "cancelled", "{job}");
    assert!(job["history"].to_string().contains("superseded"), "{job}");
}

/// A donor whose validation never ran (admission refused, exit 75) returns no verdict: the
/// job is queued again with an alarm, and nothing is ejected (#814).
#[test]
fn a_could_not_run_receipt_is_no_verdict_and_requeues_with_an_alarm() {
    let fx = Fixture::new("donor-could-not-run");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"if [ -n "$MQ_PROBE" ]; then exit 0; fi; if [ -n "$DONOR" ]; then echo COULD_NOT_RUN; exit 75; fi; exit 0"#,
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4})
    });
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    let donor_err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(donor_err.contains("could not run"), "{donor_err}");
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains("ALARM"), "{stderr}");
    assert!(stderr.contains("could not run"), "{stderr}");
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["landed"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(item_status(&fx, 1), "queued");
    let job = &jobs(&fx)[0];
    assert!(job["receipt"].is_null(), "{job}");
    assert_eq!(job["could_not_run"].as_array().unwrap().len(), 1, "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("could not run the validation (exit 75"),
        "{job}"
    );
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(status.contains("could not run: 1 receipt(s)"), "{status}");
}

#[test]
fn could_not_run_receipts_are_bounded_and_the_batch_is_refused() {
    let fx = Fixture::new("donor-could-not-run-bound");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"if [ -n "$MQ_PROBE" ]; then exit 0; fi; if [ -n "$DONOR" ]; then echo COULD_NOT_RUN; exit 75; fi; exit 0"#,
        "--offload",
        "decide",
        "--refusal-max-retries",
        "0",
    ]);
    set_config(
        &fx,
        |config| {
            config["donors"] = serde_json::json!({"max_could_not_run": 1, "claim_timeout_secs": 30})
        },
    );
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let _ = donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    let reason = report["deferred"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("could-not-run receipts (the bound)"),
        "{reason}"
    );
    assert_eq!(jobs(&fx)[0]["state"], "cancelled");
    assert_eq!(item_status(&fx, 1), "queued");
}

/// With one donor host-faulted and another known donor healthy but idle, the
/// claim timeout still fires: the host-pause hold applies only while every
/// eligible donor is faulted (the user guide's rule).
#[test]
fn a_claim_timeout_fires_when_a_known_healthy_donor_is_idle() {
    let fx = Fixture::new("donor-claim-timeout-healthy-idle");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"if [ -n "$HOSTFAULT" ]; then echo 'MQ_COULD_NOT_RUN_SCOPE: host wedged'; echo COULD_NOT_RUN; exit 75; fi; exit 0"#,
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4})
    });
    // A healthy donor lands the first change, so the store knows it.
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host-healthy");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d2", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
    // The second change's only claim is a host fault from d1; d2 stays idle.
    fx.branch("b", &[("b.txt", "b\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut run = spawn_run(&fx, &[]);
    wait_for("a queued job", Duration::from_secs(30), || {
        jobs(&fx).iter().any(|job| job["state"] == "queued")
    });
    let bad_clone = donor_clone(&fx, "donor-host-faulted");
    let out = donor(&fx, &bad_clone, "d1", &[("HOSTFAULT", "1")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The run must stop on its claim timeout (seconds), not hold for the
    // faulted donor's probes for ever.
    let start = Instant::now();
    let out = loop {
        match run.try_wait().unwrap() {
            Some(_) => break run.wait_with_output().unwrap(),
            None if start.elapsed() < Duration::from_secs(60) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            None => {
                let _ = run.kill();
                panic!("the run held for the host fault despite a healthy idle donor");
            }
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(stderr.contains("no donor took job"), "{stderr}");
    assert_eq!(item_status(&fx, 2), "queued");
}

fn host_pause_path(clone: &Path) -> PathBuf {
    clone
        .join(".git")
        .join("merge-queue-donor")
        .join("host-pause.json")
}

fn host_faults(fx: &Fixture) -> Value {
    let path = state_dir(fx).join("donor").join("host-faults.json");
    std::fs::read_to_string(path)
        .map(|text| serde_json::from_str(&text).unwrap())
        .unwrap_or(serde_json::json!({}))
}

/// Set up a donor whose validator host-refuses until `probe-ok` and
/// `validation-ok` marker files exist in the fixture root; returns its clone.
fn host_fault_fixture(tag: &str) -> (Fixture, PathBuf) {
    let fx = Fixture::new(tag);
    let run_cmd = format!(
        "if [ -n \"$MQ_PROBE\" ]; then echo probe >> {}; if [ -f {} ]; then exit 0; else exit 1; fi; fi; if [ -f {} ]; then exit 0; fi; echo 'MQ_COULD_NOT_RUN_SCOPE: host wedged'; echo COULD_NOT_RUN; exit 75",
        fx.root.join("probes").display(),
        fx.root.join("probe-ok").display(),
        fx.root.join("validation-ok").display(),
    );
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &run_cmd,
        "--offload",
        "decide",
    ]);
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host");
    (fx, clone)
}

fn probe_count(fx: &Fixture) -> usize {
    std::fs::read_to_string(fx.root.join("probes"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// A donor that restarts mid-pause must resume probing from its persisted
/// pause, clear its ordinary queue fault with one probe-passed claim, and
/// lease a job again instead of deadlocking against the queue fault.
#[test]
fn a_restarted_donor_resumes_probing_and_clears_its_host_fault() {
    let (fx, clone) = host_fault_fixture("donor-restart-mid-pause");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // First process: host-scope refusal pauses it and faults it queue-side.
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pause_path = host_pause_path(&clone);
    assert!(pause_path.exists(), "host-pause.json must be written");
    let pause: Value =
        serde_json::from_str(&std::fs::read_to_string(&pause_path).unwrap()).unwrap();
    assert_eq!(pause["reason"], "wedged", "{pause}");
    assert!(
        pause["validator"]["run"]
            .as_str()
            .is_some_and(|run| !run.is_empty()),
        "the pause must persist the validator to probe after a restart: {pause}"
    );
    assert_eq!(host_faults(&fx)["d1"]["reason"], "wedged");
    assert_eq!(
        probe_count(&fx),
        1,
        "the donor probes right after the refusal, before any restart"
    );

    // Restart (a new donor process, same work dir) with the host fixed.
    std::fs::write(fx.root.join("probe-ok"), "").unwrap();
    std::fs::write(fx.root.join("validation-ok"), "").unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(
        probe_count(&fx) >= 1,
        "the restarted donor must run MQ_PROBE=1: {stderr}"
    );
    assert!(
        !pause_path.exists(),
        "a passing probe must remove host-pause.json"
    );
    assert!(
        host_faults(&fx).get("d1").is_none(),
        "the probe-passed claim must clear the ordinary fault: {}",
        host_faults(&fx)
    );
    let (code, report, run_stderr) = finish(run);
    assert_eq!(code, Some(0), "{run_stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
}

/// An operator host hold (#1104): `host-fault --operator` survives
/// probe-passed claims and clear-faults, shows as held by operator,
/// and only plain `clear-fault` lifts it.
#[test]
fn an_operator_host_fault_is_held_until_clear_fault_lifts_it() {
    let fx = Fixture::new("donor-operator-hold");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    let out = fx.mq_ok(&[
        "donor",
        "host-fault",
        "--donor",
        "d1",
        "--reason",
        "host draining for maintenance",
        "--operator",
    ]);
    let faulted: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(faulted["faulted"], "d1", "{faulted}");
    assert_eq!(
        host_faults(&fx)["d1"]["operator"],
        true,
        "{}",
        host_faults(&fx)
    );
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(
        status.contains("held by operator: host draining for maintenance"),
        "{status}"
    );
    let fault: Value =
        serde_json::from_str(&fx.mq_ok(&["donor", "fault", "--donor", "d1"])).unwrap();
    assert_eq!(fault["fault"]["operator"], true, "{fault}");
    // A probe-passed clear-fault and a probe-passed claim both leave it.
    let out = fx.mq_ok(&["donor", "clear-fault", "--donor", "d1", "--probe-passed"]);
    assert!(out.contains("held_by_operator"), "{out}");
    assert!(host_faults(&fx).get("d1").is_some(), "{}", host_faults(&fx));
    let claimed: Value = serde_json::from_str(&fx.mq_ok(&[
        "donor",
        "claim",
        "--donor",
        "d1",
        "--jobs",
        "1",
        "--probe-passed",
    ]))
    .unwrap();
    assert!(claimed["job"].is_null(), "{claimed}");
    assert!(host_faults(&fx).get("d1").is_some(), "{}", host_faults(&fx));
    // Plain clear-fault lifts it.
    fx.mq_ok(&["donor", "clear-fault", "--donor", "d1"]);
    assert!(host_faults(&fx).get("d1").is_none(), "{}", host_faults(&fx));
    assert!(
        !fx.mq_ok(&["donor", "status"]).contains("held by operator"),
        "status still shows the lifted hold"
    );
}

/// #1059: a stale-base refusal is the job's fault, not the host's. The
/// donor must not probe (its probe here would fail), must not pause or
/// record a host fault, and the queue must record the refusal cause —
/// not the teardown line printed after it — with no could-not-run
/// strike. The same donor then takes the job normally once it can run.
#[test]
fn a_stale_base_refusal_neither_probes_nor_pauses_the_donor() {
    let fx = Fixture::new("donor-stale-base-refusal");
    let run_cmd = format!(
        "if [ -n \"$MQ_PROBE\" ]; then echo probe >> {}; exit 1; fi; \
         if [ -f {} ]; then exit 0; fi; \
         echo 'MQ_COULD_NOT_RUN_SCOPE: job stale-base target does not contain the job base'; \
         echo COULD_NOT_RUN; \
         echo 'validate-run: RETIRE-RETAINED run may still use slot s1'; exit 75",
        fx.root.join("probes").display(),
        fx.root.join("validation-ok").display(),
    );
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &run_cmd,
        "--offload",
        "decide",
    ]);
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
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
    assert_eq!(
        probe_count(&fx),
        0,
        "a job-caused stale-base refusal is never probed"
    );
    assert!(
        !host_pause_path(&clone).exists(),
        "the donor must not self-pause for a stale job"
    );
    assert!(
        host_faults(&fx).get("d1").is_none(),
        "no host fault for a stale job: {}",
        host_faults(&fx)
    );
    let job = jobs(&fx).into_iter().next().unwrap();
    assert!(
        job.get("could_not_run")
            .is_none_or(|entries| entries.as_array().is_some_and(Vec::is_empty)),
        "no could-not-run strike for a stale job: {job}"
    );
    let history = job["history"].to_string();
    assert!(
        history.contains("target does not contain the job base"),
        "the queue keeps the refusal cause: {history}"
    );
    assert!(
        !history.contains("RETIRE-RETAINED"),
        "the teardown line must not hide the cause: {history}"
    );
    // The next job is taken normally by the same donor.
    std::fs::write(fx.root.join("validation-ok"), "").unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, run_stderr) = finish(run);
    assert_eq!(code, Some(0), "{run_stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
}

/// Escalated variant: a restarted donor whose probe passes arms exactly the
/// trial the queue allows; the fault stays recorded until that trial's
/// admitted validation, which a further restart mid-pause must produce.
#[test]
fn a_restarted_donor_arms_one_trial_for_an_escalated_host_fault() {
    let (fx, clone) = host_fault_fixture("donor-restart-escalated");
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"host_fault_escalate_after": 1})
    });
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
    // Restart with the probe fixed but validation still refusing: one
    // probe-pass/launch-refuse cycle escalates the fault.
    std::fs::write(fx.root.join("probe-ok"), "").unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(probe_count(&fx) >= 1);
    let faults = host_faults(&fx);
    assert_eq!(faults["d1"]["escalated"], true, "{faults}");
    assert!(host_pause_path(&clone).exists(), "the donor pauses again");

    // Restart again mid-pause, now with validation fixed: the probe pass
    // arms a trial claim and the admitted validation clears the fault.
    std::fs::write(fx.root.join("validation-ok"), "").unwrap();
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(
        host_faults(&fx).get("d1").is_none(),
        "the trial's admitted validation must clear the escalated fault: {}",
        host_faults(&fx)
    );
    let (code, report, run_stderr) = finish(run);
    assert_eq!(code, Some(0), "{run_stderr}");
    assert_eq!(
        report["landed"][0]["prs"],
        serde_json::json!([1]),
        "{report}"
    );
}

/// A legacy host-pause.json (reason/since only, the pre-fix shape) or a
/// malformed one must not crash the donor and must be logged as unusable.
#[test]
fn a_legacy_host_pause_is_logged_and_does_not_crash_the_donor() {
    let (fx, clone) = host_fault_fixture("donor-restart-legacy-pause");
    let mut run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = run.kill();
    let _ = run.wait();
    // Downgrade the pause file to the legacy shape written before the fix.
    std::fs::write(
        host_pause_path(&clone),
        r#"{"reason":"wedged","since":"2026-11-05T20:40:05Z"}"#,
    )
    .unwrap();
    std::fs::write(fx.root.join("probe-ok"), "").unwrap();
    std::fs::write(fx.root.join("validation-ok"), "").unwrap();
    let probes_before = probe_count(&fx);
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("cannot resume probing"),
        "startup must log why a validator-less pause cannot resume probing: {stderr}"
    );
    assert_eq!(
        probe_count(&fx),
        probes_before,
        "no validator snapshot, no probe"
    );
    assert!(
        host_faults(&fx).get("d1").is_some(),
        "the queue fault stays until a probe pass or admitted validation"
    );
}

#[test]
fn compare_mode_counts_a_could_not_run_receipt_and_compares_nothing() {
    let fx = Fixture::new("donor-compare-could-not-run");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"if [ -n "$MQ_PROBE" ]; then exit 0; fi; if [ -n "$DONOR" ]; then echo COULD_NOT_RUN; exit 75; fi; sleep 8"#,
        "--offload",
        "compare",
    ]);
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let _ = donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    let (code, _, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed");
    let job = &jobs(&fx)[0];
    assert!(job["comparison"].is_null(), "{job}");
    assert_eq!(job["could_not_run"].as_array().unwrap().len(), 1, "{job}");
    let status = fx.mq_ok(&["donor", "status"]);
    assert!(status.contains("could not run: 1 receipt(s)"), "{status}");
    assert!(!status.contains("receipts match"), "{status}");
}

#[test]
fn a_donor_that_cannot_execute_the_validator_declines() {
    let fx = Fixture::new("donor-exec-missing");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        r#"if [ -n "$DONOR" ]; then mq-missing-tool-xyz; else exit 0; fi"#,
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["donors"] = serde_json::json!({"claim_timeout_secs": 4})
    });
    two_changes(&fx);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let out = donor(&fx, &clone, "d1", &[("DONOR", "1")]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("DECLINED"), "{stderr}");
    assert!(
        stderr.contains("this donor cannot run the validator")
            && stderr.contains("exit 127")
            && stderr.contains("PATH="),
        "{stderr}"
    );
    let (code, report, _) = finish(run);
    assert_eq!(code, Some(3));
    assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
    let job = &jobs(&fx)[0];
    assert!(job["receipt"].is_null(), "{job}");
    assert_eq!(job["declined_by"], serde_json::json!(["d1"]), "{job}");
}

#[test]
fn a_donor_clone_of_another_repository_refuses_to_start() {
    let fx = Fixture::new("donor-origin");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    let clone = donor_clone(&fx, "donor-host");
    let origin = git(&clone, &["remote", "get-url", "origin"])
        .trim()
        .to_owned();
    // The same repository in another form is accepted.
    git(
        &clone,
        &["remote", "set-url", "origin", &format!("file://{origin}/")],
    );
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("the same repository in another form"),
        "{stderr}"
    );
    // Another repository is refused before claiming anything.
    git(&fx.root, &["init", "-q", "--bare", "other.git"]);
    let other = fx.root.join("other.git").display().to_string();
    git(&clone, &["remote", "set-url", "origin", &other]);
    let out = donor(&fx, &clone, "d1", &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("not the queue's repository"), "{stderr}");
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

#[test]
fn compare_only_donors_compare_in_decide_mode_and_never_decide() {
    let fx = Fixture::new("donor-compare-only");
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
        config["donors"]["compare_only"] = serde_json::json!(["d2"]);
    });
    two_changes(&fx);
    let deciding_clone = donor_clone(&fx, "deciding-host");
    let compare_clone = donor_clone(&fx, "compare-host");
    let run = spawn_run(&fx, &[]);
    wait_for(
        "the deciding job and its duplicate",
        Duration::from_secs(30),
        || jobs(&fx).len() >= 2,
    );
    // The compare-only donor validates its duplicate; nothing lands on its receipt.
    let out = donor(&fx, &compare_clone, "d2", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = jobs(&fx);
    let dup = all
        .iter()
        .find(|job| job["compare_only"] == true)
        .unwrap_or_else(|| panic!("no compare-only duplicate: {all:?}"));
    assert_eq!(dup["state"], "done", "{dup}");
    assert_eq!(dup["receipt"]["donor"]["name"], "d2", "{dup}");
    assert_eq!(item_status(&fx, 1), "queued");
    // The deciding donor's receipt alone lands the batch, and the comparison is recorded.
    let out = donor(&fx, &deciding_clone, "d1", &[]);
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
    let all = jobs(&fx);
    let main = all
        .iter()
        .find(|job| job["compare_only"] != true)
        .unwrap_or_else(|| panic!("no deciding job: {all:?}"));
    assert_eq!(main["receipt"]["donor"]["name"], "d1", "{main}");
    assert_eq!(main["comparison"], "match", "{main}");
}

#[test]
fn a_job_whose_candidate_no_longer_contains_the_base_is_cancelled_at_claim_time() {
    let fx = Fixture::new("donor-stale-claim");
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
    let mut run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // The run dies without settling the job; meanwhile the base moves past its candidate.
    run.kill().unwrap();
    run.wait().unwrap();
    assert_eq!(jobs(&fx)[0]["state"], "queued");
    fx.write("moved.txt", "moved\n");
    fx.commit_all("move the base");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // A donor asking for work triggers the freshness sweep: the job is cancelled as
    // stale, never leased, and the donor is told nothing to do.
    let clone = donor_clone(&fx, "donor-host");
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let job = &jobs(&fx)[0];
    assert_eq!(job["state"], "cancelled", "{job}");
    assert!(
        job["history"]
            .to_string()
            .contains("stale: the base moved to"),
        "{job}"
    );
}

#[test]
fn an_outside_cancellation_refuses_the_batch_and_the_run_goes_on() {
    // A donor job cancelled by anything other than the run (here, an
    // external edit standing in for the stale sweep or an operator) is no
    // verdict: the batch is refused and stays queued, and the run ends
    // normally instead of failing with "cancelled outside this run".
    let fx = Fixture::new("donor-outside-cancel");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
        "--refusal-max-retries",
        "0",
        "--refusal-backoff-secs",
        "0",
    ]);
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    let id = jobs(&fx)[0]["id"].as_str().unwrap().to_owned();
    let path = job_path(&fx, &id);
    let mut job: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    job["state"] = Value::String("cancelled".to_owned());
    job["lease"] = Value::Null;
    job["history"] = serde_json::json!(["cancelled outside this run (test)"]);
    write_job(&path, &job);
    let (code, report, stderr) = finish(run);
    // Exit 3 is the refusal code (retry later), as for a could-not-run
    // bound: not the run-ending error this used to be.
    assert_eq!(code, Some(3), "{stderr}");
    assert!(
        report["validations"][0]["refused"] == true,
        "no verdict, refused: {report}"
    );
    let deferred: Vec<i64> = report["deferred"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|entry| {
            entry["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(deferred, vec![1], "{report}");
    assert_eq!(item_status(&fx, 1), "queued");
}

#[test]
fn kept_stale_roots_keep_their_donor_jobs_and_decide_after_a_landing() {
    // Three disjoint roots on one base, all validated by donors. The first
    // landing moves the base; with keep_stale_roots the other roots' donor
    // jobs must survive every claim-time sweep (each donor claim sweeps)
    // and still decide: the red single change is ejected, the green one is
    // requeued at the front with "green at <base>" and lands rebuilt.
    let fx = Fixture::new("donor-kept-roots");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let runner = r#"for t in $(ls tests); do o=pass; if grep -q FAIL "tests/$t"; then o=fail; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        runner,
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["depth"] = serde_json::json!(3);
        config["roots"] = serde_json::json!(3);
        config["keep_stale_roots"] = serde_json::json!(true);
        config["max_batch"] = serde_json::json!(1);
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let clones: Vec<(PathBuf, String)> = (1..=3)
        .map(|n| {
            (
                donor_clone(&fx, &format!("donor-host-{n}")),
                format!("d{n}"),
            )
        })
        .collect();
    let done = std::sync::atomic::AtomicBool::new(false);
    let (code, report, stderr) = std::thread::scope(|scope| {
        let _done_on_drop = DoneOnDrop(&done);
        for (clone, name) in &clones {
            scope.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = donor(&fx, clone, name, &[]);
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        let run = spawn_run(&fx, &[]);
        let outcome = finish(run);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(item_status(&fx, 1), "landed", "{report}");
    assert_eq!(item_status(&fx, 2), "ejected", "{report}");
    assert_eq!(item_status(&fx, 3), "landed", "{report}");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let third = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == 3)
        .expect("change 3 listed");
    // Change 3's root either had a donor when the base moved, and was kept: green at the old
    // base and requeued; or no donor had started it, and its job was cancelled unclaimed and
    // the change rebuilt (#915 merge-queue-keep-running-roots-only). Which, the donors' timing
    // decides.
    let changes = [("tests/a", 1), ("tests/b", 2), ("tests/c", 3)];
    let all = jobs(&fx);
    let first_c = all
        .iter()
        .filter(|job| change_of(&fx, job, &changes) == Some(3))
        .min_by_key(|job| job["id"].as_str().unwrap().to_owned())
        .expect("a job for change 3");
    if first_c["state"] == "cancelled" {
        // Under #1026 a job can be claimed (attempts 1, leased) yet
        // still unstarted for stale purposes: its donor never admitted
        // it before the base moved. That cancellation is the new
        // intended branch, distinct from an admitted job's.
        assert!(
            first_c["attempts"] == 0 || first_c.get("admitted") != Some(&Value::Bool(true)),
            "a stale root a donor had admitted was cancelled: {first_c}"
        );
        assert!(
            first_c["could_not_run"]
                .as_array()
                .is_none_or(Vec::is_empty),
            "{first_c}"
        );
    } else {
        assert!(third["history"].to_string().contains("green at"), "{third}");
    }
}

/// Which change of `changes` the donor job `job` validates: the one whose test file its
/// candidate adds to its parent (one change per batch, so earlier landings are in the parent).
/// None for a job that validates no change's candidate (a rerun on the base itself).
fn change_of(fx: &Fixture, job: &Value, changes: &[(&str, i64)]) -> Option<i64> {
    let commit = job["commit"].as_str().unwrap();
    let added = git(
        &fx.queue(),
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            &format!("{commit}^"),
            commit,
        ],
    );
    changes
        .iter()
        .find(|(file, _)| added.lines().any(|line| line.trim() == *file))
        .map(|(_, pr)| *pr)
}

/// Rewrite donor job `id`'s state as a donor holding (or no longer holding) its lease would.
fn set_lease(fx: &Fixture, id: &str, held: bool) {
    let path = job_path(fx, id);
    let mut job: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    if held {
        job["state"] = Value::String("leased".to_owned());
        job["lease"] = serde_json::json!({
            "donor": "holder", "host": "elsewhere", "token": "held-by-the-test",
            "claimed_unix": 0, "expires_unix": u64::MAX / 2,
        });
    } else {
        job["state"] = Value::String("queued".to_owned());
        job["lease"] = Value::Null;
    }
    write_job(&path, &job);
}

#[test]
fn kept_stale_roots_keep_only_running_roots_and_rebuild_unstarted_ones_on_the_new_base() {
    // #915 merge-queue-keep-running-roots-only. Three disjoint roots on one base B0, all
    // validated by donors. a is quick and lands, moving the base to B1; b is still running on
    // a donor; c's job is still queued, started by nobody. Under keep_stale_roots, b is kept
    // and decided (green at B0, requeued at the front and rebuilt), while c's job is cancelled
    // at the move with no could-not-run strike and its change rebuilt on B1: no donor ever
    // claims the B0 candidate. Before this, c's job was kept too, and the next donor validated
    // a tree whose base was no longer main; a validator that insists on a fresh base refused
    // it every time, holding a donor slot each time.
    let fx = Fixture::new("donor-kept-running-roots");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // b's validation takes a while, so it is still running when a lands.
    let runner = r#"if [ -f tests/b ]; then sleep 8; fi; for t in $(ls tests); do printf '{"id":"%s","outcome":"pass"}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        runner,
        "--offload",
        "decide",
    ]);
    set_config(&fx, |config| {
        config["depth"] = serde_json::json!(3);
        config["roots"] = serde_json::json!(3);
        config["keep_stale_roots"] = serde_json::json!(true);
        config["max_batch"] = serde_json::json!(1);
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let run = spawn_run(&fx, &[]);
    wait_for("three published root jobs", Duration::from_secs(60), || {
        jobs(&fx).len() >= 3
    });
    let changes = [("tests/a", 1), ("tests/b", 2), ("tests/c", 3)];
    let first = jobs(&fx);
    let job_of = |pr: i64| {
        first
            .iter()
            .find(|job| change_of(&fx, job, &changes) == Some(pr))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (job_a, job_b, job_c) = (job_of(1), job_of(2), job_of(3));
    // c's job is held out of reach while the two donors take a and b, then given back: queued,
    // with no lease, as a job no donor has started. Each donor then waits 30 s before asking
    // for more, long past a's landing.
    set_lease(&fx, &job_c, true);
    let clones = [
        donor_clone(&fx, "donor-host-1"),
        donor_clone(&fx, "donor-host-2"),
    ];
    let done = std::sync::atomic::AtomicBool::new(false);
    let (code, report, stderr) = std::thread::scope(|scope| {
        let _done_on_drop = DoneOnDrop(&done);
        for (n, clone) in clones.iter().enumerate() {
            let (fx, done) = (&fx, &done);
            scope.spawn(move || {
                let name = format!("d{}", n + 1);
                let clone_text = clone.display().to_string();
                let route = route(fx);
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = command(
                        &fx.root,
                        &[
                            "donor",
                            "run",
                            "--queue",
                            &route,
                            "--clone",
                            &clone_text,
                            "--name",
                            &name,
                            "--poll-secs",
                            "30",
                            "--until-idle",
                        ],
                        &[],
                    )
                    .output();
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
        }
        wait_for(
            "both donors running a and b",
            Duration::from_secs(60),
            || {
                let now = jobs(&fx);
                [&job_a, &job_b].iter().all(|id| {
                    now.iter()
                        .any(|job| job["id"] == id.as_str() && job["state"] != "queued")
                })
            },
        );
        set_lease(&fx, &job_c, false);
        let outcome = finish(run);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    assert_eq!(code, Some(0), "{stderr}");
    for pr in [1, 2, 3] {
        assert_eq!(item_status(&fx, pr), "landed", "change {pr}: {report}");
    }
    let after = jobs(&fx);
    let record = |id: &str| after.iter().find(|job| job["id"] == id).unwrap().clone();
    // The unstarted root: cancelled, never claimed by a donor, no strike against anything.
    // This is the deterministic leased-but-unadmitted case (#1026): the
    // fake lease holds the job out of every real donor's admission.
    let c = record(&job_c);
    assert_eq!(c["state"], "cancelled", "{c}");
    assert_eq!(c["attempts"], 0, "a donor claimed the stale candidate: {c}");
    assert_ne!(c.get("admitted"), Some(&Value::Bool(true)), "{c}");
    assert!(
        c["could_not_run"]
            .as_array()
            .is_none_or(|strikes| strikes.is_empty()),
        "{c}"
    );
    // ...and its change rebuilt on the new base: a second job for change 3, another candidate.
    let rebuilt: Vec<&Value> = after
        .iter()
        .filter(|job| job["id"] != job_c.as_str() && change_of(&fx, job, &changes) == Some(3))
        .collect();
    assert!(
        rebuilt.iter().any(|job| job["state"] == "done"),
        "change 3 was not validated again on the new base: {after:?}"
    );
    // The running root: kept through the move, its job done, green at B0 and requeued.
    // This is the deterministic admitted case (#1026): its donor reached
    // validator start (the 8 s sleep is inside the validator), so the
    // job was admitted before the base moved and stayed protected.
    assert_eq!(record(&job_b)["state"], "done", "{}", record(&job_b));
    assert_eq!(
        record(&job_b)["admitted"],
        Value::Bool(true),
        "{}",
        record(&job_b)
    );
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let second = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == 2)
        .expect("change 2 listed");
    assert!(
        second["history"].to_string().contains("green at"),
        "{second}"
    );
}

// --- a donor's cap, changed without a restart (#918 donor-cap-file) -----------------------------

/// Start `donor run --until-idle` for `name` from `clone` with `--jobs jobs --cap-file cap`, its
/// standard error into `log`.
fn capped_donor(
    fx: &Fixture,
    clone: &Path,
    name: &str,
    jobs: usize,
    cap: &Path,
    log: &Path,
) -> Child {
    let route = route(fx);
    let (clone, cap, jobs) = (
        clone.display().to_string(),
        cap.display().to_string(),
        jobs.to_string(),
    );
    command(
        &fx.root,
        &[
            "donor",
            "run",
            "--queue",
            &route,
            "--clone",
            &clone,
            "--name",
            name,
            "--poll-secs",
            "1",
            "--until-idle",
            "--jobs",
            &jobs,
            "--cap-file",
            &cap,
        ],
        &[],
    )
    .stdout(Stdio::null())
    .stderr(std::fs::File::create(log).unwrap())
    .spawn()
    .unwrap()
}

/// A queue of three stacked candidates, each validated slowly enough to watch, validated only by
/// donors.
fn three_slow_changes(fx: &Fixture, seconds: u32) {
    let runner = format!("sleep {seconds}; exit 0");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        &runner,
        "--offload",
        "decide",
    ]);
    set_config(fx, |config| {
        config["depth"] = serde_json::json!(3);
        config["max_batch"] = serde_json::json!(1);
        // Three stacked candidates, no root beside them: what lands is what the donors ran.
        config["disable_disjoint_root"] = serde_json::json!(true);
    });
    for (n, name) in ["a", "b", "c"].iter().enumerate() {
        fx.branch(name, &[(&format!("{name}.txt"), "x\n")]);
        fx.mq_ok(&["submit", &(n + 1).to_string(), "--head", name]);
    }
}

fn leased(fx: &Fixture) -> usize {
    jobs(fx)
        .iter()
        .filter(|job| job["state"] == "leased")
        .count()
}

#[test]
fn lowering_a_donors_cap_mid_run_holds_it_there_without_touching_running_jobs() {
    let fx = Fixture::new("donor-cap-lower");
    three_slow_changes(&fx, 4);
    let clone = donor_clone(&fx, "donor-host");
    let cap = fx.root.join("donor-cap");
    let log = fx.root.join("donor.log");
    let run = spawn_run(&fx, &[]);
    wait_for("three published jobs", Duration::from_secs(60), || {
        jobs(&fx).len() >= 3
    });
    let mut donor = capped_donor(&fx, &clone, "d1", 2, &cap, &log);
    wait_for("two jobs running", Duration::from_secs(60), || {
        leased(&fx) == 2
    });
    std::fs::write(&cap, "1\n").unwrap();
    // From the moment one of the two has reported, never more than one at once.
    let mut down = false;
    let mut most = 0;
    let deadline = Instant::now() + Duration::from_secs(120);
    while jobs(&fx)
        .iter()
        .any(|job| job["state"] != "done" && job["state"] != "cancelled")
    {
        assert!(
            Instant::now() < deadline,
            "the run never finished: {:?}",
            jobs(&fx)
        );
        let now = leased(&fx);
        if now <= 1 {
            down = true;
        }
        if down {
            most = most.max(now);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, report, stderr) = finish(run);
    let _ = donor.wait();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"].as_array().map(Vec::len),
        Some(3),
        "{report}"
    );
    assert_eq!(most, 1, "the donor ran more jobs than its cap");
    let said = std::fs::read_to_string(&log).unwrap();
    assert!(
        said.contains(&format!("cap now 1 (was 2) from {}", cap.display())),
        "{said}"
    );
    // No running job was given back: every job a donor took, it took once and reported.
    for job in jobs(&fx) {
        assert!(job["attempts"].as_u64() <= Some(1), "{job}");
        if job["state"] == "done" {
            assert_eq!(job["attempts"], 1, "{job}");
        }
        assert!(!job["history"].to_string().contains("gave"), "{job}");
    }
    assert!(
        fx.mq_ok(&["donor", "status"]).contains("donor d1: cap 1"),
        "status shows no cap"
    );
}

#[test]
fn a_cap_of_zero_drains_without_exiting_and_clearing_it_resumes_and_a_malformed_cap_is_ignored() {
    let fx = Fixture::new("donor-cap-drain");
    three_slow_changes(&fx, 3);
    let clone = donor_clone(&fx, "donor-host");
    let cap = fx.root.join("donor-cap");
    let log = fx.root.join("donor.log");
    // Malformed: --jobs stands, and the donor says why.
    std::fs::write(&cap, "lots\n").unwrap();
    let run = spawn_run(&fx, &[]);
    wait_for("three published jobs", Duration::from_secs(60), || {
        jobs(&fx).len() >= 3
    });
    let mut donor = capped_donor(&fx, &clone, "d1", 1, &cap, &log);
    wait_for("one job running", Duration::from_secs(60), || {
        leased(&fx) == 1
    });
    // Drain with a job in flight: it finishes and reports; nothing more is claimed, and the
    // donor stays, though --until-idle would otherwise end it with nothing running.
    std::fs::write(&cap, "0").unwrap();
    wait_for("the running job reported", Duration::from_secs(60), || {
        jobs(&fx).iter().any(|job| job["state"] == "done") && leased(&fx) == 0
    });
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(leased(&fx), 0, "a draining donor claimed: {:?}", jobs(&fx));
    assert!(
        jobs(&fx).iter().any(|job| job["state"] == "queued"),
        "the premise is work left: {:?}",
        jobs(&fx)
    );
    assert!(
        donor.try_wait().unwrap().is_none(),
        "a draining donor exited"
    );
    assert!(
        fx.mq_ok(&["donor", "status"])
            .contains("donor d1: cap 0 (draining)"),
        "{}",
        fx.mq_ok(&["donor", "status"])
    );
    // Cleared: --jobs again on the next poll, and the rest is validated.
    std::fs::remove_file(&cap).unwrap();
    let (code, report, stderr) = finish(run);
    let status = donor.wait().unwrap();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(status.success());
    assert_eq!(
        report["landed"].as_array().map(Vec::len),
        Some(3),
        "{report}"
    );
    let said = std::fs::read_to_string(&log).unwrap();
    assert!(
        said.contains("ignoring cap file") && said.contains("\"lots\""),
        "{said}"
    );
    assert!(said.contains("cap now 0 (was 1)"), "{said}");
    assert!(said.contains("cap now 1 (was 0)"), "{said}");
    for job in jobs(&fx) {
        assert!(
            job["attempts"].as_u64() <= Some(1),
            "a job was given back: {job}"
        );
    }
}

/// A validator change on disk mid-run reaches donor jobs published from
/// the next plan-intake on (#1050): the earlier job keeps the old
/// validator, the later job carries the new one, and the run records a
/// `config-reload` event. No batch mixes validators because jobs are
/// published per batch.
#[test]
fn a_validator_change_mid_run_reaches_later_donor_jobs() {
    let fx = Fixture::new("donor-config-reload");
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--offload",
        "decide",
    ]);
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let clone = donor_clone(&fx, "donor-host");
    let run = spawn_run(&fx, &[]);
    wait_for("the first published job", Duration::from_secs(30), || {
        jobs(&fx).len() == 1
    });
    // Change the validator on disk, then submit a second change so the
    // run takes intake in and reloads the config.
    set_config(&fx, |config| {
        config["validator"]["run"] = serde_json::json!("exit 0 # v2");
    });
    fx.branch("b", &[("b.txt", "b\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    wait_for("a second published job", Duration::from_secs(60), || {
        jobs(&fx).len() >= 2
    });
    // Read the job files directly: `donor status` does not carry the
    // validator each job was published with.
    let jobs_dir = state_dir(&fx).join("donor").join("jobs");
    let published: Vec<Value> = std::fs::read_dir(&jobs_dir)
        .unwrap()
        .flatten()
        .map(|entry| serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap())
        .collect();
    let runs: Vec<String> = published
        .iter()
        .map(|job| job["validator"]["run"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        runs.iter().any(|run| run == "exit 0") && runs.iter().any(|run| run == "exit 0 # v2"),
        "jobs carry both validators: {runs:?}"
    );
    let digests: Vec<&str> = published
        .iter()
        .map(|job| job["tool"]["validator_digest"].as_str().unwrap())
        .collect();
    assert!(
        digests.iter().any(|d| *d != digests[0]),
        "the later job's digest must differ: {digests:?}"
    );
    let out = donor(&fx, &clone, "d1", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, report, stderr) = finish(run);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        report["landed"].as_array().map(Vec::len),
        Some(2),
        "{report}"
    );
    // The run recorded the reload in its events.
    let runs_dir = state_dir(&fx).join("runs");
    let mut events = String::new();
    for entry in std::fs::read_dir(&runs_dir).unwrap().flatten() {
        if let Ok(text) = std::fs::read_to_string(entry.path().join("events.jsonl")) {
            events.push_str(&text);
        }
    }
    assert!(events.contains("config-reload"), "{events}");
}

/// Claim-time revision gate (#1105): a claim declaring a revision the
/// queue host was not built from is refused without leasing; a
/// matching declaration, and an old claim with none, lease as today.
#[test]
fn a_claim_declaring_a_mismatched_revision_is_refused_without_leasing() {
    let fx = Fixture::new("donor-claim-revision");
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
    let mut run = spawn_run(&fx, &[]);
    wait_for("a published job", Duration::from_secs(30), || {
        !jobs(&fx).is_empty()
    });
    // Claims go through `command` (which sets MERGE_QUEUE_ALLOW_DIRTY,
    // as donor runs do): this test build is dirty, so a matching
    // declaration must carry --revision-dirty too.
    let claim = |args: &[&str]| -> Value {
        let out = command(&fx.queue(), args, &[]).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap()
    };
    // An old-style claim with no declaration leases as today; give
    // the job back so the gated claims below see it queued again.
    let claimed = claim(&["donor", "claim", "--donor", "d1", "--jobs", "1"]);
    assert!(!claimed["job"].is_null(), "{claimed}");
    assert!(claimed.get("refused").is_none(), "{claimed}");
    let id = claimed["job"]["id"].as_str().unwrap().to_owned();
    let token = claimed["job"]["lease"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (ok, _, err) = donor_call(&fx, "release", &id, &token);
    assert!(ok, "{err}");
    assert_eq!(jobs(&fx)[0]["state"], "queued", "{}", jobs(&fx)[0]);
    let wrong = "1".repeat(40);
    assert_ne!(wrong, own_revision());
    let refused = claim(&[
        "donor",
        "claim",
        "--donor",
        "d1",
        "--jobs",
        "1",
        "--revision",
        &wrong,
    ]);
    assert!(refused["job"].is_null(), "{refused}");
    assert!(
        refused["refused"]
            .as_str()
            .is_some_and(|why| why.contains("this donor's merge-queue is built from")),
        "{refused}"
    );
    // Nothing was leased: the job is still queued for a matching donor.
    assert_eq!(jobs(&fx)[0]["state"], "queued", "{}", jobs(&fx)[0]);
    // A matching declaration leases it. This test build is dirty
    // (its checkout has the test's own changes in it), which a real
    // donor binary declares with --revision-dirty.
    let revision = own_revision();
    let version_out = Command::new(bin()).arg("--version").output().unwrap();
    let dirty = String::from_utf8_lossy(&version_out.stdout).contains("(dirty)");
    let mut matching = vec![
        "donor",
        "claim",
        "--donor",
        "d1",
        "--jobs",
        "1",
        "--revision",
        revision.as_str(),
    ];
    if dirty {
        matching.push("--revision-dirty");
    }
    let claimed = claim(&matching);
    assert!(!claimed["job"].is_null(), "{claimed}");
    assert!(claimed.get("refused").is_none(), "{claimed}");
    let _ = run.kill();
    let _ = run.wait();
}

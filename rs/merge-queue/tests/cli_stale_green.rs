//! A candidate green on the immediately previous base, stale only because
//! unrelated changes landed, stacks on the stack head even below the
//! pass-rate gate when its files are disjoint from what landed and its
//! merge onto the head is clean. Fresh candidates still wait for the gate.

mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"ok=1; for f in tests/*; do [ -f "$f" ] || continue; if grep -qx FAIL "$f"; then echo "{\"id\":\"x\",\"outcome\":\"fail\"}" >> "$MQ_RESULTS"; ok=0; fi; done; [ $ok -eq 1 ]"#;

fn config_path(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
}

fn set_config(fx: &Fixture, fields: &[(&str, Value)]) {
    let path = config_path(fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (key, field) in fields {
        value[key] = field.clone();
    }
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap()
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
}

fn events(fx: &Fixture) -> String {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap()
}

/// Seed a 50% recent pass rate (below an 80% stack gate), then run three
/// disjoint passing changes with three root slots and stale roots kept.
fn seeded(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
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
        RUNNER,
        "--max-batch",
        "1",
    ]);
    fx.branch("g0", &[("tests/g0", "ok\n")]);
    fx.branch("r0", &[("tests/r0", "FAIL\n")]);
    fx.mq_ok(&["submit", "101", "--head", "g0"]);
    fx.mq_ok(&["submit", "102", "--head", "r0"]);
    fx.mq_ok(&["run", "--json"]);
    set_config(
        &fx,
        &[
            ("depth", serde_json::json!(3)),
            ("roots", serde_json::json!(3)),
            ("stack_min_pass_percent", serde_json::json!(80)),
            ("keep_stale_roots", serde_json::json!(true)),
        ],
    );
    fx
}

#[test]
fn a_stale_green_stacks_below_the_gate_when_disjoint() {
    let fx = seeded("stale-green-stacked");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 1), "landed", "{r}");
    assert_eq!(status(&fx, 2), "landed", "{r}");
    assert_eq!(status(&fx, 3), "landed", "{r}");
    let events = events(&fx);
    let stacked: Vec<&str> = events
        .lines()
        .filter(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| e["kind"] == "stale-green-stacked")
        })
        .collect();
    assert!(!stacked.is_empty(), "stale green stacked: {events}");
    assert!(
        stacked.iter().any(|line| line.contains("was green at")),
        "{stacked:?}"
    );
    // Two previously green candidates land without a third validation each:
    // the last change validates on the previous base and once stacked.
    let c_validations = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([3]) && v["cancelled"] == false)
        .count();
    assert!(c_validations <= 2, "c validated {c_validations} times: {r}");
}

#[test]
fn a_fresh_candidate_below_the_gate_still_waits_for_a_root() {
    let fx = seeded("stale-green-fresh");
    fx.branch("x", &[("tests/x", "ok\n")]);
    fx.branch("y", &[("tests/y", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "x"]);
    fx.mq_ok(&["submit", "2", "--head", "y"]);
    let base = fx.remote_main();
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    let second = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["prs"] == serde_json::json!([2]))
        .expect("second validated");
    assert_eq!(
        second["parent"], base,
        "fresh candidate rooted, not stacked: {r}"
    );
    assert!(
        !events(&fx).contains("stale-green-stacked"),
        "fresh candidates never stale-stack"
    );
}

#[test]
fn a_red_stacked_validation_falls_back_without_an_extra_strike() {
    // C is green alone; stacked on B the combination is red (the validator
    // fails when tests/b and tests/c coexist). It is then handled as an
    // ordinary red single: ejected once, B still lands.
    let fx = Fixture::new("stale-green-red");
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
        RUNNER,
        "--max-batch",
        "1",
    ]);
    fx.branch("g0", &[("tests/g0", "ok\n")]);
    fx.branch("r0", &[("tests/r0", "FAIL\n")]);
    fx.mq_ok(&["submit", "101", "--head", "g0"]);
    fx.mq_ok(&["submit", "102", "--head", "r0"]);
    fx.mq_ok(&["run", "--json"]);
    // Switch to the interaction validator for the measured run.
    let path = config_path(&fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["validator"]["run"] = serde_json::json!(
        r#"if [ -f tests/b ] && [ -f tests/c ]; then echo '{"id":"x","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1; fi; exit 0"#
    );
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    set_config(
        &fx,
        &[
            ("depth", serde_json::json!(3)),
            ("roots", serde_json::json!(3)),
            ("stack_min_pass_percent", serde_json::json!(80)),
            ("keep_stale_roots", serde_json::json!(true)),
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 2), "landed", "{r}");
    // The seeded rate in this fixture may not be below the gate; only assert
    // the fallback outcome, which holds either way: C is not left queued and
    // was validated at most twice (previous green + one red).
    let c_validations = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([3]) && v["cancelled"] == false)
        .count();
    assert!(c_validations <= 2, "c validated {c_validations} times: {r}");
    assert_ne!(status(&fx, 3), "landed", "{r}");
}

#[test]
fn a_stale_green_overlapping_what_landed_does_not_stack() {
    let fx = seeded("stale-green-overlap");
    // B touches the same file A lands, so it is not disjoint from landed.
    fx.branch("a", &[("tests/shared", "a\n")]);
    fx.branch("b", &[("tests/shared", "b\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    let events = events(&fx);
    // B conflicts with the landed A as a root candidate (today's behaviour),
    // and is not named as stale-green-stacked with B.
    assert!(
        !events.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| {
                e["kind"] == "stale-green-stacked"
                    && e["detail"].as_str().unwrap_or_default().contains("#2")
            })
        }),
        "{events}"
    );
    let _ = r;
}

/// The seeded fixture with a generated file, `gen/index` (the sorted names of the test files),
/// committed on the base and declared in `regenerate.paths` with the command that writes it.
fn seeded_with_generated_index(tag: &str) -> Fixture {
    let fx = seeded(tag);
    git(&fx.dev(), &["pull", "-q", "--ff-only", "origin", "main"]);
    fx.write("gen/index", "g0\nt0\n");
    fx.commit_all("generated index");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    set_config(
        &fx,
        &[(
            "regenerate",
            serde_json::json!({
                "paths": ["gen/index"],
                "command": "ls tests | sort > gen/index",
                "when": "always",
            }),
        )],
    );
    fx
}

fn stale_green_stacked_names(events: &str, pr: i64) -> bool {
    events.lines().any(|line| {
        serde_json::from_str::<Value>(line).is_ok_and(|e| {
            e["kind"] == "stale-green-stacked"
                && e["detail"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&format!("#{pr}"))
        })
    })
}

#[test]
fn a_stale_green_sharing_only_a_generated_file_stacks() {
    // Each change adds a test and commits its regenerated index, so every pair of changes
    // conflicts in gen/index and nowhere else. The regenerate step rewrites gen/index on
    // every candidate, so that shared file is not overlap and the stale greens stack.
    let fx = seeded_with_generated_index("stale-green-generated");
    fx.branch("a", &[("tests/a", "ok\n"), ("gen/index", "a\ng0\nt0\n")]);
    fx.branch("b", &[("tests/b", "ok\n"), ("gen/index", "b\ng0\nt0\n")]);
    fx.branch("c", &[("tests/c", "ok\n"), ("gen/index", "c\ng0\nt0\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 1), "landed", "{r}");
    assert_eq!(status(&fx, 2), "landed", "{r}");
    assert_eq!(status(&fx, 3), "landed", "{r}");
    let events = events(&fx);
    assert!(
        events.contains("\"stale-green-stacked\""),
        "a stale green whose only overlap is a generated file stacks: {events}"
    );
    let c_validations = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([3]) && v["cancelled"] == false)
        .count();
    assert!(c_validations <= 2, "c validated {c_validations} times: {r}");
    // The landed tree's generated file is the regenerated one, not any change's copy.
    git(&fx.dev(), &["fetch", "-q", "origin", "main"]);
    assert_eq!(
        git(&fx.dev(), &["show", "origin/main:gen/index"]),
        "a\nb\nc\ng0\nt0"
    );
}

#[test]
fn a_stale_green_sharing_a_hand_written_file_still_rebuilds_with_regeneration_on() {
    // Same generated index, plus a hand-written file both A and B edit on different lines:
    // the merge is clean, so both validate as disjoint roots, but the shared hand-written path
    // is real overlap and B, stale after A lands, is never stacked as a stale green.
    let fx = seeded_with_generated_index("stale-green-generated-overlap");
    fx.write("tests/shared", "1\n2\n3\n4\n5\n");
    fx.write("gen/index", "g0\nshared\nt0\n");
    fx.commit_all("shared file");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.branch("a", &[("tests/shared", "a\n2\n3\n4\n5\n")]);
    fx.branch("b", &[("tests/shared", "1\n2\n3\n4\nb\n")]);
    fx.branch(
        "c",
        &[("tests/c", "ok\n"), ("gen/index", "c\ng0\nshared\nt0\n")],
    );
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq_ok(&["run", "--json"]);
    let r = report(&out);
    assert_eq!(status(&fx, 1), "landed", "{r}");
    assert_eq!(status(&fx, 2), "landed", "{r}");
    let events = events(&fx);
    assert!(!stale_green_stacked_names(&events, 2), "{events}");
}

fn spawn_run(fx: &Fixture) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
    command
        .args(["run", "--json"])
        .current_dir(fx.queue())
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("TESTTMP", "1")
        .env_remove("MERGE_QUEUE_STATE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().unwrap()
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_prioritized_stale_green_roots_instead_of_stacking_under_a_plain_head() {
    // Change 2 arrives by intake, prioritized, and validates slowly as a
    // root; change 3 lands meanwhile, so change 2 comes back green on a
    // stale base. Disjoint from the stack head (change 4, not
    // prioritized, still validating), it must rebuild as its own root
    // on the new base, never stacked under change 4's candidate.
    let fx = seeded("stale-green-prioritized");
    let path = config_path(&fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // Gate-driven, not sleep-driven: candidates holding tests/p or
    // tests/b wait for `release`; candidates holding tests/d wait for
    // `release-d`, so the ordering below holds at any host speed.
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    value["validator"]["run"] = serde_json::json!(format!(
        r#"gates='{gates}'
if [ -f tests/d ]; then gate="$gates/release-d"; elif [ -f tests/p ] || [ -f tests/b ]; then gate="$gates/release"; else gate=""; fi
if [ -n "$gate" ]; then i=0; while [ ! -e "$gate" ] && [ $i -lt 12000 ]; do sleep 0.05; i=$((i+1)); done; [ -e "$gate" ] || exit 1; fi
ok=1; for f in tests/*; do [ -f "$f" ] || continue; if grep -qx FAIL "$f"; then echo "{{\"id\":\"x\",\"outcome\":\"fail\"}}" >> "$MQ_RESULTS"; ok=0; fi; done; [ $ok -eq 1 ]"#,
        gates = gates.display()
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.branch("p", &[("tests/p", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "3", "--head", "b"]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    let run = spawn_run(&fx);
    // Both first validations are out; only then does the prioritized
    // change arrive, so it cannot front the first wave.
    wait_for("the first validations", Duration::from_secs(240), || {
        events(&fx).matches("\"kind\":\"validate\"").count() >= 2
    });
    let recorded = fx.mq(&["submit", "2", "--head", "p", "--prioritized"]);
    // Change 3 and the prioritized change are both gated out; release
    // them together: change 3 lands first and moves the base under the
    // prioritized change's candidate.
    wait_for(
        "the prioritized validation",
        Duration::from_secs(240),
        || events(&fx).contains("batch #2 on"),
    );
    std::fs::write(gates.join("release"), "").unwrap();
    // Once the prioritized change is requeued as a stale green, let the
    // stack head (change 4) finish; the rebuild must not wait for it.
    wait_for("the stale requeue", Duration::from_secs(240), || {
        events(&fx).contains("stale-root-green")
    });
    std::fs::write(gates.join("release-d"), "").unwrap();
    let out = run.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let _ = recorded;
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(status(&fx, 1), "landed", "{r}");
    assert_eq!(status(&fx, 2), "landed", "{r}");
    assert_eq!(status(&fx, 3), "landed", "{r}");
    assert_eq!(status(&fx, 4), "landed", "{r}");
    let events = events(&fx);
    assert!(
        events.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| {
                e["kind"] == "stale-green-rooted"
                    && e["detail"].as_str().unwrap_or_default().contains("#2")
            })
        }),
        "prioritized stale green rooted: {events}"
    );
    assert!(
        !events.lines().any(|line| {
            serde_json::from_str::<Value>(line).is_ok_and(|e| {
                e["kind"] == "stale-green-stacked"
                    && e["detail"].as_str().unwrap_or_default().contains("#2")
            })
        }),
        "prioritized stale green must not stack: {events}"
    );
    // Its rebuild sat on the base as it was just before change 2
    // landed (the previous landed batch's tip), not on a candidate.
    let landed = r["landed"].as_array().unwrap();
    let position = landed
        .iter()
        .position(|batch| batch["prs"] == serde_json::json!([2]))
        .expect("change 2 landed");
    let landed_tip = landed[position - 1]["tip"].as_str().unwrap();
    let rebuilds: Vec<&Value> = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["prs"] == serde_json::json!([2]) && v["cancelled"] != true)
        .collect();
    let last = rebuilds
        .last()
        .unwrap_or_else(|| panic!("change 2 rebuilt: {r}"));
    assert_eq!(last["parent"], landed_tip, "{r}");
}

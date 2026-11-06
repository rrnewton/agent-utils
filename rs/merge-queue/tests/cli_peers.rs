//! Cross-repository ordering: a change requires a peer repository's change, which lands first
//! as its exact commit; the dependent never lands when the peer change does not.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const RUNNER: &str = r#"rc=0; for t in $(ls tests 2>/dev/null); do o=pass; grep -q FAIL "tests/$t" && { o=fail; rc=1; }; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit $rc"#;

fn seeded(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx
}

fn init(fx: &Fixture, extra: &[&str]) {
    let mut args = vec!["init", "--validator", "command", "--validator-run", RUNNER];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap()
}

#[test]
fn the_required_peer_change_lands_first_and_exactly() {
    let peer = seeded("peer-lib");
    let main = seeded("peer-app");
    init(&peer, &[]);
    let peer_state = peer.queue().join(".git/merge-queue");
    init(&main, &["--peer", &format!("lib={}", peer_state.display())]);

    // The library change, and an application change pinning it (by requiring its exact head).
    let lib_head = peer.branch("lib-feature", &[("tests/lib", "ok\n")]);
    peer.mq_ok(&["submit", "7", "--head", "lib-feature"]);
    main.branch(
        "app-feature",
        &[("tests/app", "ok\n"), ("lib.pin", &format!("{lib_head}\n"))],
    );
    main.mq_ok(&[
        "submit",
        "1",
        "--head",
        "app-feature",
        "--requires",
        "lib#7",
    ]);

    let plan: Value = serde_json::from_str(&main.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "waiting-on-peer");
    assert!(plan["held"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("lib #7 is queued"));
    // The peer change is now marked exact: it lands as its own commit, alone.
    let peer_items: Value = serde_json::from_str(&peer.mq_ok(&["list", "--json"])).unwrap();
    assert_eq!(peer_items[0]["exact"], true);

    let out = main.mq_ok(&["run", "--with-peers", "--json"]);
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["peer_runs"][0]["peer"], "lib");
    assert_eq!(r["peer_runs"][0]["landed"], serde_json::json!([7]));
    assert_eq!(
        peer.remote_main(),
        lib_head,
        "the peer landed the exact pinned commit"
    );
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([1]));
    assert_eq!(status(&main, 1), "landed");
}

#[test]
fn a_dependent_never_lands_when_its_peer_change_is_ejected() {
    let peer = seeded("peer-lib-red");
    let main = seeded("peer-app-red");
    init(&peer, &[]);
    let peer_state = peer.queue().join(".git/merge-queue");
    init(&main, &["--peer", &format!("lib={}", peer_state.display())]);
    peer.branch("lib-broken", &[("tests/lib", "FAIL\n")]);
    peer.mq_ok(&["submit", "8", "--head", "lib-broken"]);
    main.branch("app-needs", &[("tests/app", "ok\n")]);
    main.mq_ok(&["submit", "2", "--head", "app-needs", "--requires", "lib#8"]);
    let before = main.remote_main();

    let r: Value = serde_json::from_str(&main.mq_ok(&["run", "--with-peers", "--json"])).unwrap();
    assert_eq!(r["peer_runs"][0]["ejected"], serde_json::json!([8]), "{r}");
    assert!(r["landed"].as_array().unwrap().is_empty());
    assert_eq!(main.remote_main(), before);
    assert_eq!(status(&main, 2), "queued");
    let plan: Value = serde_json::from_str(&main.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("lib #8 is ejected"));
}

#[test]
fn an_exact_change_that_is_not_on_the_base_is_ejected_not_replayed() {
    let peer = seeded("peer-lib-stale");
    let main = seeded("peer-app-stale");
    init(&peer, &[]);
    let peer_state = peer.queue().join(".git/merge-queue");
    init(&main, &["--peer", &format!("lib={}", peer_state.display())]);
    peer.branch("lib-old", &[("tests/lib", "ok\n")]);
    peer.mq_ok(&["submit", "9", "--head", "lib-old"]);
    main.branch("app-pin", &[("tests/app", "ok\n")]);
    main.mq_ok(&["submit", "3", "--head", "app-pin", "--requires", "lib#9"]);
    // The peer's base moves on, so the pinned commit can no longer land as-is.
    peer.write("other", "x\n");
    peer.commit_all("unrelated");
    git(&peer.dev(), &["push", "-q", "origin", "HEAD:main"]);

    let r: Value = serde_json::from_str(&main.mq_ok(&["run", "--with-peers", "--json"])).unwrap();
    assert_eq!(r["peer_runs"][0]["ejected"], serde_json::json!([9]));
    assert!(r["landed"].as_array().unwrap().is_empty());
    assert_eq!(status(&main, 3), "queued");
}

/// Hold a queue's writer lock, as a peer mid-run does, until the
/// returned file is dropped (the `cli_intake` pattern).
fn hold_lock(state: &std::path::Path) -> std::fs::File {
    use fs2::FileExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("lock"))
        .unwrap();
    file.try_lock_exclusive().unwrap();
    file
}

fn intake_files(state: &std::path::Path, kind: &str) -> Vec<Value> {
    let dir = state.join("intake").join(kind);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Ok(value) = serde_json::from_str(&text) {
                    out.push(value);
                }
            }
        }
    }
    out
}

#[test]
fn a_requirement_against_a_busy_peer_is_recorded_and_applied_next_cycle() {
    let peer = seeded("peer-lib-busy");
    let main = seeded("peer-app-busy");
    init(&peer, &[]);
    let peer_state = peer.queue().join(".git/merge-queue");
    init(&main, &["--peer", &format!("lib={}", peer_state.display())]);
    let lib_head = peer.branch("lib-feature", &[("tests/lib", "ok\n")]);
    peer.mq_ok(&["submit", "7", "--head", "lib-feature"]);
    main.branch("app-feature", &[("tests/app", "ok\n")]);

    // The peer is mid-run: its writer lock is held for the whole run, and
    // resolving the requirement must not fail the submit (it used to).
    let holder = hold_lock(&peer_state);
    let submitted = main.mq(&[
        "submit",
        "1",
        "--head",
        "app-feature",
        "--requires",
        "lib#7",
    ]);
    assert!(
        submitted.status.success(),
        "submit against a busy peer succeeds: {}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    // A mark-exact request waits in the peer's intake, against this head.
    let pending = intake_files(&peer_state, "pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["request"]["op"], "mark-exact", "{pending:?}");
    assert_eq!(pending[0]["request"]["head"], lib_head, "{pending:?}");
    // The requirement resolved to the item's current head, and the
    // dependent is held waiting-on-peer until the peer lands it.
    let items: Value = serde_json::from_str(&main.mq_ok(&["list", "--all", "--json"])).unwrap();
    let dependent = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == 1)
        .expect("dependent recorded (possibly as pending intake)");
    let _ = dependent;
    let plan: Value = serde_json::from_str(&main.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "waiting-on-peer", "{plan}");
    drop(holder);

    // The peer's next cycle applies mark-exact before planning, so the
    // change is exact and lands as its own commit...
    let out = main.mq_ok(&["run", "--with-peers", "--json"]);
    let r: Value = serde_json::from_str(&out).unwrap();
    let peer_items: Value =
        serde_json::from_str(&peer.mq_ok(&["list", "--all", "--json"])).unwrap();
    let peer_item = peer_items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == 7)
        .expect("peer item listed");
    assert_eq!(peer_item["exact"], true, "{peer_items} run={r}");
    assert_eq!(peer.remote_main(), lib_head, "peer landed the exact head");
    // ...and only then does the dependent land.
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([1]), "{r}");
    assert_eq!(status(&main, 1), "landed");
}

#[test]
fn a_mark_exact_whose_head_moved_is_refused_and_noted() {
    let peer = seeded("peer-lib-moved");
    let main = seeded("peer-app-moved");
    init(&peer, &[]);
    let peer_state = peer.queue().join(".git/merge-queue");
    init(&main, &["--peer", &format!("lib={}", peer_state.display())]);
    peer.branch("lib-feature", &[("tests/lib", "ok\n")]);
    peer.mq_ok(&["submit", "7", "--head", "lib-feature"]);
    main.branch("app-feature", &[("tests/app", "ok\n")]);
    let holder = hold_lock(&peer_state);
    let submitted = main.mq(&[
        "submit",
        "1",
        "--head",
        "app-feature",
        "--requires",
        "lib#7",
    ]);
    assert!(submitted.status.success());
    drop(holder);

    // The peer change's head moves before the request is applied.
    let new_head = peer.branch("lib-feature-2", &[("tests/lib2", "ok\n")]);
    let item_path = peer_state.join("items/7.json");
    let mut item: Value =
        serde_json::from_str(&std::fs::read_to_string(&item_path).unwrap()).unwrap();
    item["head"] = serde_json::json!(new_head);
    std::fs::write(&item_path, serde_json::to_string_pretty(&item).unwrap()).unwrap();

    // The peer's next run applies intake first: the request is refused
    // and noted in the item's history, and the change is not exact.
    peer.mq_ok(&["run", "--json"]);
    let refused = intake_files(&peer_state, "refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0]["error"].as_str().unwrap().contains("head moved"),
        "{refused:?}"
    );
    let items: Value = serde_json::from_str(&peer.mq_ok(&["list", "--all", "--json"])).unwrap();
    assert!(items[0]["exact"] != true, "{items}");
    let history: Vec<String> = items[0]["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        history.contains(&"intake-refused".to_owned()),
        "{history:?}"
    );
}

/// Push branch `name`: a merge, made on `main` as the remote has it now, of another history
/// (unrelated, with its own test file). Returns the merge and the other history's tip.
fn import_merge(fx: &Fixture, name: &str) -> (String, String) {
    let dev = fx.dev();
    git(&dev, &["fetch", "-q", "origin"]);
    git(
        &dev,
        &["checkout", "-q", "--orphan", &format!("{name}-other")],
    );
    git(&dev, &["rm", "-rfq", "."]);
    fx.write("imported/README", "another history\n");
    fx.write("tests/imported", "ok\n");
    fx.commit_all("another history");
    let other = git(&dev, &["rev-parse", "HEAD"]);
    git(&dev, &["checkout", "-q", "-B", name, "origin/main"]);
    git(
        &dev,
        &[
            "merge",
            "-q",
            "--allow-unrelated-histories",
            "-m",
            "import another history",
            &other,
        ],
    );
    git(
        &dev,
        &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
    );
    let merge = git(&dev, &["rev-parse", "HEAD"]);
    git(&dev, &["checkout", "-q", "main"]);
    (merge, other)
}

#[test]
fn an_exact_merge_on_the_base_lands_with_its_own_id_and_both_parents() {
    let fx = seeded("exact-merge");
    init(&fx, &[]);
    let help = fx.mq_ok(&["submit", "--help"]);
    assert!(
        help.contains("--exact") && help.contains("preserve-merge"),
        "{help}"
    );
    let base = fx.remote_main();
    let (merge, other) = import_merge(&fx, "import");
    fx.mq_ok(&["submit", "5", "--head", "import", "--exact"]);
    // Recorded on the item, so every later command (a restarted daemon too) sees it.
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--json"])).unwrap();
    assert_eq!(items[0]["exact"], true, "{items}");

    let r: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([5]), "{r}");
    assert_eq!(
        fx.remote_main(),
        merge,
        "the merge landed as itself, not a copy"
    );
    let remote = fx.remote();
    assert_eq!(git(&remote, &["rev-parse", "main^1"]), base);
    assert_eq!(git(&remote, &["rev-parse", "main^2"]), other);
    git(&remote, &["merge-base", "--is-ancestor", &other, "main"]);
    assert_eq!(status(&fx, 5), "landed");
}

#[test]
fn an_exact_merge_is_ejected_not_replayed_once_the_base_moves() {
    let fx = seeded("exact-merge-stale");
    init(&fx, &[]);
    let (merge, other) = import_merge(&fx, "import");
    // The alias, and a base that moves on before the merge lands.
    fx.mq_ok(&["submit", "6", "--head", "import", "--preserve-merge"]);
    fx.write("unrelated", "x\n");
    fx.commit_all("unrelated");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let moved = fx.remote_main();

    let r: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(fx.remote_main(), moved, "nothing landed");
    assert_eq!(status(&fx, 6), "ejected");
    let remote = fx.remote();
    // Nothing of the other history was replayed onto the base.
    assert_eq!(
        git(&remote, &["ls-tree", "--name-only", "main", "imported"]),
        ""
    );
    let reachable = std::process::Command::new("git")
        .arg("-C")
        .arg(&remote)
        .args(["merge-base", "--is-ancestor", &other, "main"])
        .status()
        .unwrap();
    assert!(!reachable.success());
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let history = items[0]["history"].to_string();
    assert!(history.contains("not on top of"), "{history}");
    assert_ne!(fx.remote_main(), merge);
}

#[test]
fn a_merge_head_without_exact_is_still_replayed_as_its_own_commits() {
    let fx = seeded("merge-replayed");
    init(&fx, &[]);
    let dev = fx.dev();
    let side = fx.branch("side", &[("tests/side", "ok\n")]);
    git(&dev, &["checkout", "-q", "-B", "merged", "main"]);
    fx.write("tests/merged", "ok\n");
    fx.commit_all("merged: own change");
    git(&dev, &["merge", "-q", "--no-ff", "-m", "merge side", &side]);
    git(&dev, &["push", "-q", "-f", "origin", "HEAD:merged"]);
    let merge = git(&dev, &["rev-parse", "HEAD"]);
    git(&dev, &["checkout", "-q", "main"]);
    let base = fx.remote_main();
    fx.mq_ok(&["submit", "7", "--head", "merged"]);

    let r: Value = serde_json::from_str(&fx.mq_ok(&["run", "--json"])).unwrap();
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([7]), "{r}");
    let remote = fx.remote();
    assert_ne!(fx.remote_main(), merge, "replayed, so new commits");
    assert_eq!(
        git(&remote, &["rev-list", "--merges", &format!("{base}..main")]),
        "",
        "replayed linear, as before: no merge commit lands"
    );
    assert_eq!(
        git(&remote, &["rev-list", "--count", &format!("{base}..main")]),
        "2"
    );
    for test in ["tests/side", "tests/merged"] {
        assert_eq!(
            git(&remote, &["ls-tree", "--name-only", "main", test]),
            test
        );
    }
}

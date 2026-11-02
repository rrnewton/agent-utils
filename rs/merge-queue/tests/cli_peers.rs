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

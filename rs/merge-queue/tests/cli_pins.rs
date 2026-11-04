//! Monotonic pins (`pins`): a commit of another repository pinned at a gitlink and in files
//! that repeat its hash. When a change moves the pin and the target pins another commit, the
//! descendant wins at every site, in either direction; diverged pins, a partial update and a pin
//! off the pinned repository's branch are refused by name; a change that touches no pin site
//! is never checked.

mod support;

use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{git, Fixture};

/// The pinned repository's commits.
struct Dep {
    url: PathBuf,
    /// `d1` -> `d2` -> `d3` on main.
    d: [String; 3],
    /// Branched from `d2` and merged into main after `d3`: on main, but neither an ancestor
    /// nor a descendant of `d3`.
    x: String,
    /// Only on the branch `side`, never on main.
    y: String,
}

fn commit(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", name]);
    git(dir, &["rev-parse", "HEAD"])
}

fn dep(fx: &Fixture) -> Dep {
    let work = fx.root.join("dep-work");
    git(&fx.root, &["init", "-q", "-b", "main", "dep-work"]);
    let d1 = commit(&work, "d1");
    let d2 = commit(&work, "d2");
    git(&work, &["checkout", "-q", "-b", "x"]);
    let x = commit(&work, "x");
    git(&work, &["checkout", "-q", "main"]);
    let d3 = commit(&work, "d3");
    git(&work, &["merge", "-q", "--no-ff", "-m", "merge x", "x"]);
    git(&work, &["checkout", "-q", "-b", "side", "main"]);
    let y = commit(&work, "y");
    git(&work, &["checkout", "-q", "main"]);
    git(&fx.root, &["clone", "-q", "--bare", "dep-work", "dep.git"]);
    Dep {
        url: fx.root.join("dep.git"),
        d: [d1, d2, d3],
        x,
        y,
    }
}

/// The pinned files, carrying `pin`.
fn pinned_files(pin: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "a/Cargo.toml",
            format!("[dependencies]\ndep = {{ git = \"dep\", rev = \"{pin}\" }}\nserde = \"1\"\n"),
        ),
        (
            "Cargo.lock",
            format!("[[package]]\nname = \"dep\"\nsource = \"git+dep?rev={pin}#{pin}\"\n"),
        ),
        (
            "c/Cargo.toml",
            format!("[dependencies]\ndep = {{ git = \"dep\", rev = \"{pin}\" }}\n"),
        ),
    ]
}

/// Commit on `branch` (from `from`): the gitlink at `gitlink_pin`, each pinned file at its
/// own pin (`file_pins`, default `gitlink_pin`), and `extra` files; push it; return its head.
fn pin_commit(
    fx: &Fixture,
    branch: &str,
    from: &str,
    gitlink_pin: &str,
    file_pins: &[(&str, &str)],
    extra: &[(&str, &str)],
) -> String {
    let dev = fx.dev();
    git(&dev, &["checkout", "-q", "-B", branch, from]);
    std::fs::create_dir_all(dev.join("dep")).unwrap();
    git(
        &dev,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{gitlink_pin},dep"),
        ],
    );
    for (path, _) in pinned_files(gitlink_pin) {
        let pin = file_pins
            .iter()
            .find(|(file, _)| *file == path)
            .map_or(gitlink_pin, |(_, pin)| *pin);
        let text = pinned_files(pin)
            .into_iter()
            .find(|(file, _)| *file == path)
            .unwrap()
            .1;
        fx.write(path, &text);
        git(&dev, &["add", path]);
    }
    for (path, text) in extra {
        fx.write(path, text);
        git(&dev, &["add", path]);
    }
    git(&dev, &["commit", "-q", "-m", &format!("{branch}: pin")]);
    git(
        &dev,
        &["push", "-q", "-f", "origin", &format!("HEAD:{branch}")],
    );
    let head = git(&dev, &["rev-parse", "HEAD"]);
    git(&dev, &["checkout", "-q", "main"]);
    head
}

/// A queue whose base pins `d1`, with `pins` configured against `url`.
fn queue(tag: &str) -> (Fixture, Dep) {
    let fx = Fixture::new(tag);
    let dep = dep(&fx);
    fx.write("b/Cargo.toml", "[dependencies]\nserde = \"1\"\n");
    git(&fx.dev(), &["add", "b/Cargo.toml"]);
    pin_commit(&fx, "main", "main", &dep.d[0], &[], &[]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    set_pins(&fx, &dep.url.display().to_string());
    (fx, dep)
}

fn set_pins(fx: &Fixture, url: &str) {
    let path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["pins"] = serde_json::json!([{
        "name": "dep",
        "gitlink": "dep",
        "files": ["*Cargo.toml", "Cargo.lock"],
        "url": url,
    }]);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

fn run(fx: &Fixture) -> Value {
    let out = fx.mq(&["run", "--json"]);
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|error| {
        panic!(
            "bad report ({error}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// The gitlink and the pinned files on the remote's main.
fn main_pins(fx: &Fixture) -> (String, String, String) {
    let remote = fx.remote();
    let tree = git(&remote, &["ls-tree", "main", "--", "dep"]);
    let gitlink = tree
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .to_owned();
    let show = |path: &str| git(&remote, &["show", &format!("main:{path}")]);
    (gitlink, show("a/Cargo.toml"), show("Cargo.lock"))
}

fn landed(report: &Value) -> Vec<i64> {
    report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|landing| landing["prs"].as_array().unwrap().clone())
        .map(|pr| pr.as_i64().unwrap())
        .collect()
}

fn ejection(report: &Value, pr: i64) -> String {
    report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["pr"] == pr)
        .map(|e| e["reason"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("#{pr} was not ejected: {report}"))
}

#[test]
fn the_change_with_the_descendant_pin_wins() {
    let (fx, dep) = queue("pins-change-wins");
    let [d1, d2, d3] = dep.d.clone();
    pin_commit(&fx, "up2", "main", &d2, &[], &[("u1.txt", "1\n")]);
    pin_commit(&fx, "up3", "main", &d3, &[], &[("u2.txt", "2\n")]);
    fx.mq_ok(&["submit", "1", "--head", "up2"]);
    fx.mq_ok(&["submit", "2", "--head", "up3"]);
    let report = run(&fx);
    // One batch: the two pin moves do not conflict as far as planning is concerned.
    assert_eq!(landed(&report), vec![1, 2], "{report}");
    assert_eq!(report["landed"].as_array().unwrap().len(), 1, "{report}");
    let (gitlink, manifest, lock) = main_pins(&fx);
    assert_eq!(gitlink, d3);
    assert!(manifest.contains(&format!("rev = \"{d3}\"")), "{manifest}");
    assert!(lock.contains(&format!("?rev={d3}#{d3}")), "{lock}");
    assert!(
        !manifest.contains(&d1) && !manifest.contains(&d2),
        "{manifest}"
    );
    // Both changes' other work landed; an unpinned manifest was left alone.
    let files = git(&fx.remote(), &["ls-tree", "--name-only", "main"]);
    assert!(
        files.contains("u1.txt") && files.contains("u2.txt"),
        "{files}"
    );
    let resolutions = report["pin_resolutions"].as_array().unwrap();
    assert_eq!(resolutions.len(), 2, "{report}");
    assert_eq!(resolutions[1]["pr"], 2, "{report}");
    assert_eq!(resolutions[1]["target"], d2.as_str(), "{report}");
    assert_eq!(resolutions[1]["change"], d3.as_str(), "{report}");
    assert_eq!(resolutions[1]["chosen"], d3.as_str(), "{report}");
    assert_eq!(resolutions[1]["sites"], 4, "{report}");
}

#[test]
fn the_target_with_the_descendant_pin_wins() {
    let (fx, dep) = queue("pins-target-wins");
    let [_, d2, d3] = dep.d.clone();
    pin_commit(&fx, "up3", "main", &d3, &[], &[]);
    // The older pin also edits a line of a pinned file next to its pin: kept.
    let dev = fx.dev();
    pin_commit(&fx, "up2", "main", &d2, &[], &[]);
    git(&dev, &["checkout", "-q", "up2"]);
    let manifest = std::fs::read_to_string(dev.join("a/Cargo.toml")).unwrap();
    fx.write(
        "a/Cargo.toml",
        &manifest.replace("serde = \"1\"", "serde = \"2\""),
    );
    git(&dev, &["commit", "-q", "-am", "up2: newer serde"]);
    git(&dev, &["push", "-q", "-f", "origin", "HEAD:up2"]);
    git(&dev, &["checkout", "-q", "main"]);
    fx.mq_ok(&["submit", "1", "--head", "up3"]);
    fx.mq_ok(&["submit", "2", "--head", "up2"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![1, 2], "{report}");
    let (gitlink, manifest, lock) = main_pins(&fx);
    assert_eq!(gitlink, d3, "the pin moved backwards");
    assert!(manifest.contains(&format!("rev = \"{d3}\"")), "{manifest}");
    assert!(manifest.contains("serde = \"2\""), "{manifest}");
    assert!(lock.contains(&format!("?rev={d3}#{d3}")), "{lock}");
    let last = report["pin_resolutions"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["pr"], 2, "{report}");
    assert_eq!(last["target"], d3.as_str(), "{report}");
    assert_eq!(last["change"], d2.as_str(), "{report}");
    assert_eq!(last["chosen"], d3.as_str(), "{report}");
    let markdown = std::fs::read_to_string(
        fx.queue()
            .join(".git/merge-queue/runs")
            .join(report["run"].as_str().unwrap())
            .join("report.md"),
    )
    .unwrap();
    assert!(markdown.contains("## Pin resolutions"), "{markdown}");
}

#[test]
fn diverged_pins_are_refused() {
    let (fx, dep) = queue("pins-diverged");
    pin_commit(&fx, "up3", "main", &dep.d[2], &[], &[]);
    pin_commit(&fx, "upx", "main", &dep.x, &[], &[]);
    fx.mq_ok(&["submit", "1", "--head", "up3"]);
    fx.mq_ok(&["submit", "2", "--head", "upx"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![1], "{report}");
    let reason = ejection(&report, 2);
    assert!(
        reason.contains("diverged") && reason.contains("neither is an ancestor"),
        "{reason}"
    );
    assert_eq!(main_pins(&fx).0, dep.d[2]);
}

#[test]
fn a_partial_update_is_refused() {
    let (fx, dep) = queue("pins-partial");
    let [d1, d2, _] = dep.d.clone();
    // The gitlink and the manifest move; the lock file stays at the old pin.
    pin_commit(&fx, "half", "main", &d2, &[("Cargo.lock", &d1)], &[]);
    fx.mq_ok(&["submit", "1", "--head", "half"]);
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    let reason = ejection(&report, 1);
    assert!(
        reason.contains("partial update")
            && reason.contains(&format!("Cargo.lock is at {}", &d1[..10])),
        "{reason}"
    );
    assert_eq!(main_pins(&fx).0, d1);
}

#[test]
fn a_pin_that_is_not_on_the_branch_is_refused() {
    let (fx, dep) = queue("pins-off-branch");
    pin_commit(&fx, "upy", "main", &dep.y, &[], &[]);
    fx.mq_ok(&["submit", "1", "--head", "upy"]);
    let report = run(&fx);
    assert!(landed(&report).is_empty(), "{report}");
    let reason = ejection(&report, 1);
    assert!(reason.contains("main"), "{reason}");
    assert!(
        reason.contains("is not on") && reason.contains(&dep.y[..10]),
        "{reason}"
    );
}

#[test]
fn a_change_that_touches_no_pin_site_is_never_checked() {
    let (fx, dep) = queue("pins-untouched");
    // Nothing can be fetched from here: a check would fail.
    set_pins(
        &fx,
        &fx.root.join("no-such-repository").display().to_string(),
    );
    fx.branch("docs", &[("README", "docs\n")]);
    fx.mq_ok(&["submit", "1", "--head", "docs"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![1], "{report}");
    assert!(report.get("pin_resolutions").is_none(), "{report}");
    assert_eq!(main_pins(&fx).0, dep.d[0]);
}

#[test]
fn a_real_conflict_in_a_declared_file_is_still_a_conflict() {
    // Declared files settle like generated ones while combining, so a real conflict in one,
    // outside any pin, must still refuse the second change rather than keep the target's side.
    let (fx, _) = queue("pins-real-conflict");
    fx.branch("s2", &[("b/Cargo.toml", "[dependencies]\nserde = \"2\"\n")]);
    fx.branch("s3", &[("b/Cargo.toml", "[dependencies]\nserde = \"3\"\n")]);
    fx.mq_ok(&["submit", "1", "--head", "s2"]);
    fx.mq_ok(&["submit", "2", "--head", "s3"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![1], "{report}");
    let reason = ejection(&report, 2);
    assert!(reason.contains("conflicts on b/Cargo.toml"), "{reason}");
    let manifest = git(&fx.remote(), &["show", "main:b/Cargo.toml"]);
    assert!(manifest.contains("serde = \"2\""), "{manifest}");
}

#[test]
fn a_change_deleting_a_pinned_file_keeps_it_deleted() {
    // The target moved the pin in a file the change deletes: only the pin changed there, so
    // the deletion stands instead of the file coming back at the new pin.
    let (fx, dep) = queue("pins-deleted-file");
    pin_commit(&fx, "up2", "main", &dep.d[1], &[], &[]);
    let dev = fx.dev();
    git(&dev, &["checkout", "-q", "-B", "drop-c", "main"]);
    git(&dev, &["rm", "-q", "c/Cargo.toml"]);
    git(&dev, &["commit", "-q", "-m", "drop c"]);
    git(&dev, &["push", "-q", "-f", "origin", "HEAD:drop-c"]);
    git(&dev, &["checkout", "-q", "main"]);
    fx.mq_ok(&["submit", "1", "--head", "up2"]);
    fx.mq_ok(&["submit", "2", "--head", "drop-c"]);
    let report = run(&fx);
    assert_eq!(landed(&report), vec![1, 2], "{report}");
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(!files.contains("c/Cargo.toml"), "{files}");
    assert_eq!(main_pins(&fx).0, dep.d[1]);
}

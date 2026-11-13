//! A precheck-refusal notice sent by `submit` addresses the project
//! registry, not a registry resolved from the submitter's working
//! directory (#1472 mq-notice-registry-from-cwd): `init` records the
//! nearest `.agentctl` above its working directory, and the submit
//! below runs from a directory outside the project entirely.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use support::{git, Fixture};

/// Every test file passes unless it says FAIL.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

/// Fails when a test file says BADFMT.
const FORMAT_CHECK: &str = r#"bad=$(grep -l BADFMT tests/* 2>/dev/null)
[ -z "$bad" ] && exit 0
for f in $bad; do echo "Diff in $PWD/$f:1:"; echo "-BADFMT"; echo "+ok"; done
exit 1"#;

#[test]
fn submit_from_outside_the_project_notices_the_project_registry() {
    let fx = Fixture::new("submit-registry");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // The project registry lives at the project root (the fixture root);
    // the queue clone is one level below it.
    let registry = fx.root.join(".agentctl");
    std::fs::create_dir_all(&registry).unwrap();
    // A fake agentctl recording its argv, and a refusing precheck.
    let sends = fx.root.join("fake-agentctl.log");
    let fake = fx.root.join("fake-agentctl.sh");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            sends.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let script = fx.root.join("precheck.sh");
    std::fs::write(&script, format!("{FORMAT_CHECK}\n")).unwrap();
    let precheck_arg = format!("sh '{}'", script.display());
    let fake_arg = fake.to_string_lossy().into_owned();
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--precheck",
        &precheck_arg,
        "--agentctl",
        &fake_arg,
    ]);
    // The init recorded the project registry in the queue config.
    let config = std::fs::read_to_string(fx.queue().join(".git/merge-queue/config.json")).unwrap();
    assert!(
        config.contains(&registry.display().to_string()),
        "init records the project registry: {config}"
    );

    fx.branch("b", &[("tests/b", "BADFMT\n")]);
    // Submit from a working directory outside the project, naming the
    // queue explicitly: the refusal notice must still address the
    // recorded project registry, not `<outside>/.agentctl`.
    let outside = fx.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let queue = fx.queue().display().to_string();
    let out = Command::new(env!("CARGO_BIN_EXE_merge-queue"))
        .args([
            "--repo", &queue, "submit", "1", "--head", "b", "--notify", "watcher",
        ])
        .current_dir(&outside)
        .env("GIT_AUTHOR_NAME", "Dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.com")
        .env("GIT_COMMITTER_NAME", "Dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("TESTTMP", "1")
        .env_remove("MERGE_QUEUE_STATE")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let log = std::fs::read_to_string(&sends).unwrap_or_default();
    let expected = format!("--registry {} send ", registry.display());
    assert!(
        log.lines().any(|line| line.starts_with(&expected)),
        "the notice addresses the project registry ({expected}): {log}"
    );
}

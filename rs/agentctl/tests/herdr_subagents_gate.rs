//! The production launch gate is the `herdr-subagents` binary's own
//! `--chat-launch-gate` entry. The in-crate daemon test must compile a
//! gate double, because a unit-test binary cannot be that entry — so
//! this integration test is the one place that drives the REAL
//! shipped binary as both runner and gate, end to end over one fake
//! Codex turn.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentctl::foreign::lifecycle::{enqueue_message_at, stop_path_at, transcript_path_at};
use agentctl::foreign::registry::{
    processed_dir_at, read_registry_at, with_registry_lock_at, AgentRecord,
};

fn fake_base(test: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("agentctl-gate-bin-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn real_binary_gates_a_fake_codex_turn_end_to_end() {
    use std::os::unix::fs::PermissionsExt as _;
    let binary = Path::new(env!("CARGO_BIN_EXE_herdr-subagents"));
    let base = fake_base("e2e");
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    with_registry_lock_at(&base, |agents| {
        agents.insert(
            "worker-1".to_string(),
            AgentRecord {
                name: "worker-1".to_string(),
                harness: "codex".to_string(),
                backend: "tmux".to_string(),
                cwd: work.to_string_lossy().into_owned(),
                status: "idle".to_string(),
                next_seq: 1,
                created_at: "2026-11-07T00:00:00Z".to_string(),
                ..AgentRecord::default()
            },
        );
        Ok(())
    })
    .unwrap();
    let fake = base.join("fake-codex.sh");
    std::fs::write(
        &fake,
        "#!/bin/bash\n\
         out=\"\"; prev=\"\"\n\
         for a in \"$@\"; do if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi; prev=\"$a\"; done\n\
         cat >/dev/null\n\
         printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"sess-fake-1\"}'\n\
         printf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"worked\"}}'\n\
         printf 'fake answer\\n' > \"$out\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    enqueue_message_at(&base, "worker-1", "do the thing", None).unwrap();
    // No gate override anywhere: the runner defaults to spawning
    // `<current_exe> --chat-launch-gate`, i.e. this very binary.
    let mut handle = std::process::Command::new(binary)
        .args(["__runner", "worker-1"])
        .env("HERDR_SUBAGENTS_HOME", &base)
        .env("CODEX_BIN", &fake)
        .spawn()
        .expect("spawn real herdr-subagents runner");
    let deadline = Instant::now() + Duration::from_secs(30);
    let done = loop {
        let processed = processed_dir_at(&base, "worker-1")
            .unwrap()
            .join("000000000001.json");
        let status = read_registry_at(&base)
            .ok()
            .and_then(|agents| agents.get("worker-1").map(|rec| rec.status.clone()));
        if processed.exists() && status.as_deref() == Some("idle") {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    std::fs::write(stop_path_at(&base, "worker-1").unwrap(), "").unwrap();
    let outcome = handle.wait().expect("runner process");
    assert!(done, "turn did not complete; runner exit: {outcome:?}");
    assert!(outcome.success(), "runner process failed: {outcome:?}");
    let transcript =
        std::fs::read_to_string(transcript_path_at(&base, "worker-1").unwrap()).unwrap();
    assert!(
        transcript.contains("===TURN 1 OUTPUT===\nfake answer"),
        "{transcript}"
    );
    assert!(transcript.contains("===TURN-DONE 1 rc=0"), "{transcript}");
    assert!(
        transcript.contains("===RUNNER DOWN worker-1"),
        "{transcript}"
    );
    let record = &read_registry_at(&base).unwrap()["worker-1"];
    assert_eq!(record.session_id.as_deref(), Some("sess-fake-1"));
    let _ = std::fs::remove_dir_all(&base);
}

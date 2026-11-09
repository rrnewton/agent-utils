//! The TUI launch loop must retry the transient `agent_not_found`
//! Herdr answers until it classifies a just-launched pane (~0.5 s
//! live) instead of treating the first probe error as terminal
//! before its own 5 s deadline.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use agentctl::foreign::lifecycle::launch_herdr_tui_at;

fn fake_base(test: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("agentctl-tui-launch-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn tui_launch_retries_agent_not_found_until_classified() {
    use std::os::unix::fs::PermissionsExt as _;
    let base = fake_base("retry");
    let calls = base.join("agent-get-calls");
    let fake = base.join("fake-herdr.sh");
    std::fs::write(
        &fake,
        "#!/bin/bash\n\
         case \"$*\" in\n\
         \"workspace list\") echo '{\"result\":{\"workspaces\":[{\"label\":\"subagents\",\"workspace_id\":\"wT\"}]}}' ;;\n\
         \"tab create\"*) echo '{\"result\":{\"tab\":{\"tab_id\":\"wT:t9\"},\"root_pane\":{\"pane_id\":\"wT:p9\"}}}' ;;\n\
         \"pane run\"*) ;;\n\
         \"tab rename\"*) echo '{\"result\":{}}' ;;\n\
         \"pane process-info --pane wT:p9\") printf '{\"result\":{\"process_info\":{\"foreground_processes\":[{\"name\":\"codex\",\"pid\":%s,\"argv\":[\"codex\"]}]}}}' \"$FAKE_CODEX_PID\" ;;\n\
         \"agent get wT:p9\")\n\
           count=0; [ -f \"$FAKE_CALLS\" ] && count=$(cat \"$FAKE_CALLS\"); count=$((count+1)); echo \"$count\" > \"$FAKE_CALLS\"\n\
           if [ \"$count\" -lt 3 ]; then echo '{\"error\":{\"code\":\"agent_not_found\",\"message\":\"agent target wT:p9 not found\"}}' >&2; exit 1; fi\n\
           echo '{\"result\":{\"agent\":{\"agent_status\":\"idle\"}}}' ;;\n\
         *) echo \"fake herdr: unsupported $*\" >&2; exit 2 ;;\n\
         esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // HERDR_BIN is read from the process environment by the library;
    // this integration binary runs only this test, so setting it
    // here cannot race a sibling test.
    std::env::set_var("HERDR_BIN", &fake);
    std::env::set_var("FAKE_CALLS", &calls);
    std::env::set_var("FAKE_CODEX_PID", std::process::id().to_string());
    std::env::remove_var("SUBAGENTS_HERDR_WORKSPACE");
    let cwd = base.to_string_lossy().into_owned();
    let (tab_id, pane_id, probe) = launch_herdr_tui_at(
        &base,
        Path::new("/proc"),
        "worker-1",
        &cwd,
        None,
        None,
        Some(false),
    )
    .expect("launch must retry agent_not_found until classified");
    assert_eq!((tab_id.as_str(), pane_id.as_str()), ("wT:t9", "wT:p9"));
    assert!(probe.codex_alive);
    let agent_gets: i64 = std::fs::read_to_string(&calls)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(agent_gets >= 3, "agent get calls: {agent_gets}");
    let _ = std::fs::remove_dir_all(&base);
}

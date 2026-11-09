//! `agent read` answers with the scrollback text itself, not a JSON
//! envelope (herdr 0.9.5 observed live). The TUI scrollback reader
//! must take stdout verbatim instead of demanding JSON.

#![cfg(target_os = "linux")]

use std::path::PathBuf;

use agentctl::foreign::lifecycle::read_agent_output_at;
use agentctl::foreign::registry::{with_registry_lock_at, AgentRecord};

fn fake_base(test: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("agentctl-tui-read-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn tui_read_takes_agent_read_stdout_verbatim() {
    use std::os::unix::fs::PermissionsExt as _;
    let base = fake_base("verbatim");
    with_registry_lock_at(&base, |agents| {
        agents.insert(
            "worker-1".to_string(),
            AgentRecord {
                name: "worker-1".to_string(),
                harness: "codex".to_string(),
                backend: "herdr".to_string(),
                mode: "tui".to_string(),
                presentation_pane: Some("wT:p9".to_string()),
                status: "idle".to_string(),
                cwd: base.to_string_lossy().into_owned(),
                created_at: "2026-11-07T00:00:00Z".to_string(),
                ..AgentRecord::default()
            },
        );
        Ok(())
    })
    .unwrap();
    let fake = base.join("fake-herdr.sh");
    std::fs::write(
        &fake,
        "#!/bin/bash\n\
         case \"$*\" in\n\
         \"pane process-info --pane wT:p9\") printf '{\"result\":{\"process_info\":{\"foreground_processes\":[{\"name\":\"codex\",\"pid\":%s,\"argv\":[\"codex\"]}]}}}' \"$FAKE_CODEX_PID\" ;;\n\
         \"agent get wT:p9\") echo '{\"result\":{\"agent\":{\"agent_status\":\"idle\"}}}' ;;\n\
         \"agent read wT:p9\"*) printf 'scrollback-body\\nsecond line\\n' ;;\n\
         *) echo \"fake herdr: unsupported $*\" >&2; exit 2 ;;\n\
         esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // HERDR_BIN is read from the process environment by the library;
    // this integration binary runs only this test, so setting it
    // here cannot race a sibling test.
    std::env::set_var("HERDR_BIN", &fake);
    std::env::set_var("FAKE_CODEX_PID", std::process::id().to_string());
    let result = read_agent_output_at(&base, "worker-1", "all", None, None)
        .expect("plain-text agent read must be accepted verbatim");
    assert!(
        result.text.contains("scrollback-body"),
        "read text: {:?}",
        result.text
    );
    let _ = std::fs::remove_dir_all(&base);
}

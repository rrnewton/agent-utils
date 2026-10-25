//! End-to-end tmux backend test against a real tmux server on a private
//! `-L` socket. Skips (passes) when tmux is not installed.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use term_backend::{Multiplexer, PaneTerminal, ScreenRead, TmuxBackend};

fn tmux_bin() -> Option<PathBuf> {
    let bin = PathBuf::from("tmux");
    term_backend::executable_available(&bin).then_some(bin)
}

#[test]
fn tmux_backend_drives_a_real_private_server() {
    let Some(bin) = tmux_bin() else {
        eprintln!("tmux not installed; skipping");
        return;
    };
    let socket = format!("term-backend-test-{}", std::process::id());
    let backend = TmuxBackend::new(bin.clone(), Some(socket.clone()));
    let cleanup = |bin: &PathBuf, socket: &str| {
        let _ = Command::new(bin)
            .args(["-L", socket, "kill-server"])
            .output();
    };
    cleanup(&bin, &socket);

    let result = (|| -> term_backend::Result<()> {
        assert_eq!(backend.backend_name(), "tmux");
        assert!(!backend.capabilities().native_agent_status);
        let created = backend.create_workspace("tb-test", std::path::Path::new("/tmp"), &[])?;
        assert_eq!(backend.list_workspaces()?.len(), 1);
        assert_eq!(backend.list_tabs(&created.workspace_id)?.len(), 1);
        let panes = backend.list_panes(None)?;
        assert_eq!(panes.len(), 1);
        let info = backend.pane_info(&created.pane_id)?;
        assert_eq!(info.id, created.pane_id);
        assert!(info.cwd.is_some(), "pane_current_path expected: {info:?}");
        assert!(
            info.pid.is_some_and(|pid| pid > 0),
            "pane_pid expected: {info:?}"
        );
        assert_eq!(info.agent_status, None);

        backend.paste_text(&created.pane_id, "echo term-backend-marker")?;
        backend.send_keys(&created.pane_id, "Enter")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut screen = String::new();
        while Instant::now() < deadline {
            screen = backend.read_screen(&created.pane_id, ScreenRead::recent(50))?;
            // The marker must appear twice: echoed input plus command output.
            if screen.matches("term-backend-marker").count() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            screen.matches("term-backend-marker").count() >= 2,
            "command output not seen on screen: {screen:?}"
        );

        let second = backend.create_tab(
            &created.workspace_id,
            "second",
            std::path::Path::new("/tmp"),
            &[],
        )?;
        backend.rename_tab(&second.tab_id, "renamed")?;
        assert!(backend
            .list_tabs(&created.workspace_id)?
            .iter()
            .any(|tab| tab.label == "renamed"));
        backend.focus_pane(&second.pane_id)?;
        backend.close_tab(&second.tab_id)?;
        assert_eq!(backend.list_panes(None)?.len(), 1);
        backend.close_workspace(&created.workspace_id)?;
        assert!(backend.list_workspaces()?.is_empty());
        Ok(())
    })();
    cleanup(&bin, &socket);
    result.unwrap();
}

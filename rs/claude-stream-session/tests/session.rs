//! The persistent driver against a fake `claude` that speaks stream-json.
//!
//! The fake answers each input line with one assistant text block and a success
//! `result`, so the test proves warm reuse (one process, many turns) and token
//! accounting without a real model call.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use claude_stream_session::session::{Session, SessionConfig};

fn fake_claude() -> (tempfile_dir::Dir, String) {
    let dir = tempfile_dir::Dir::new("claude-stream-fake");
    let path = dir.path().join("fake-claude");
    let script = "#!/bin/sh\n\
while IFS= read -r line; do\n\
  printf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"one line summary\"}]}}'\n\
  printf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"one line summary\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}'\n\
done\n";
    std::fs::File::create(&path)
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = path.to_string_lossy().into_owned();
    (dir, program)
}

mod tempfile_dir {
    use std::path::PathBuf;

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn warm_session_reuses_one_process_across_turns() {
    let (_dir, program) = fake_claude();
    let mut session = Session::spawn(SessionConfig {
        program,
        ..SessionConfig::default()
    })
    .unwrap();
    let first = session.complete("summarise this").unwrap();
    assert_eq!(first.text, "one line summary");
    assert_eq!(first.tokens, Some(15));
    let second = session.complete("summarise that").unwrap();
    assert_eq!(second.text, "one line summary");
    assert_eq!(session.turns(), 2);
    assert_eq!(session.tokens(), 30);
    assert!(session.alive());
}

#[test]
fn missing_program_fails_by_name() {
    let error = Session::spawn(SessionConfig {
        program: "/nonexistent/claude".to_owned(),
        ..SessionConfig::default()
    })
    .err()
    .expect("spawn must fail");
    assert!(error.to_string().contains("could not start"), "{error}");
}

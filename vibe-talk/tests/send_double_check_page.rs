//! Runs `tests/js/send_double_check.test.mjs` under `cargo test`. `#621 send-double-check`.

use std::path::PathBuf;
use std::process::Command;

const SUITE: &str = "tests/js/send_double_check.test.mjs";

#[test]
fn the_send_double_check_page_suite_passes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("node")
        .current_dir(&root)
        .arg("--test")
        .arg(SUITE)
        .output()
        .unwrap_or_else(|error| panic!("could not run node, which {SUITE} needs: {error}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "the send-double-check page suite failed:\n{text}"
    );
}

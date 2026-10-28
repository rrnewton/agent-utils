//! Runs `tests/js/page_build.test.mjs` under `cargo test`: the page build (`web/build.mjs`) writes
//! hashed, minified assets whose maps resolve to the right source lines, and a shell naming them.
//! `#624 voice-page-modularity`.
//!
//! The build runs the esbuild that `contract/package-lock.json` pins, so `contract/node_modules`
//! must be installed first: `npm ci --prefix contract` (validation's `setup.contract-tools`).

use std::path::PathBuf;
use std::process::Command;

const SUITE: &str = "tests/js/page_build.test.mjs";

#[test]
fn the_page_build_suite_passes() {
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
        "the page build suite failed:\n{text}"
    );
}

//! Runs the page modules' own suites under `cargo test`: each module below `web/src/` held directly,
//! built as the page build builds it, with no page around it. `#624 voice-page-modularity`.
//!
//! A module's suite sits at `tests/js/<layer>/<area>.test.mjs`, after `web/src/<layer>/<area>/`.
//! They are listed by name, not found by a glob, so the validation graph's inventory check
//! (py/tests/test_validation_graph_contract.py) can hold every suite file to exactly one harness: a
//! new file that nothing lists fails there rather than never running.
//!
//! Each file builds its module with the esbuild that `contract/package-lock.json` pins, so
//! `contract/node_modules` must be installed first: `npm ci --prefix contract`.

use std::path::PathBuf;
use std::process::Command;

const SUITES: &[&str] = &[
    "tests/js/app/application.test.mjs",
    "tests/js/app/keyboard.test.mjs",
    "tests/js/app/server.test.mjs",
    "tests/js/core/annotations.test.mjs",
    "tests/js/core/api.test.mjs",
    "tests/js/core/call.test.mjs",
    "tests/js/core/call-devices.test.mjs",
    "tests/js/core/channels.test.mjs",
    "tests/js/core/navigation.test.mjs",
    "tests/js/core/outbox.test.mjs",
    "tests/js/core/settings.test.mjs",
    "tests/js/core/search.test.mjs",
    "tests/js/core/speech.test.mjs",
    "tests/js/core/sync.test.mjs",
    "tests/js/foundation/index.test.mjs",
    "tests/js/platform/web.test.mjs",
    "tests/js/present/call.test.mjs",
    "tests/js/present/notices.test.mjs",
    "tests/js/present/rows.test.mjs",
    "tests/js/present/settings.test.mjs",
    "tests/js/present/time.test.mjs",
    "tests/js/view/channel.test.mjs",
    "tests/js/view/channel-scope.test.mjs",
    "tests/js/view/channel-freshread.test.mjs",
    "tests/js/view/channel-freshness.test.mjs",
    "tests/js/view/channel-readstate.test.mjs",
    "tests/js/view/channel-loading-done.test.mjs",
    "tests/js/view/channel-columnstate.test.mjs",
    "tests/js/view/channel-smallwords.test.mjs",
    "tests/js/view/channel-textselection.test.mjs",
    "tests/js/view/channel-next11.test.mjs",
    "tests/js/view/channel-next12.test.mjs",
    "tests/js/view/channel-gathering.test.mjs",
    "tests/js/view/voice-watchframe.test.mjs",
    "tests/js/view/voice-readingacts.test.mjs",
    "tests/js/view/readcore.test.mjs",
    "tests/js/view/jumpanchor.test.mjs",
    "tests/js/view/composer.test.mjs",
    "tests/js/view/documents.test.mjs",
    "tests/js/view/catchup.test.mjs",
    "tests/js/view/dom.test.mjs",
    "tests/js/view/settings.test.mjs",
    "tests/js/view/shell-navigation.test.mjs",
    "tests/js/view/shell-channelselection.test.mjs",
    "tests/js/view/shell-deployment.test.mjs",
    "tests/js/view/shell-channeldirectory.test.mjs",
    "tests/js/view/settings-editors.test.mjs",
    "tests/js/view/voice.test.mjs",
];

/// How many tests the suites define between them. Raise it with them: it is a floor against a
/// suite that stopped COLLECTING, so left behind it would keep passing while a file went missing.
const COLLECTED_FLOOR: usize = 405;

#[test]
fn the_page_module_suites_pass() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("node")
        .current_dir(&root)
        .arg("--test")
        // The assertions below parse TAP; Node's default reporter varies by version.
        .arg("--test-reporter=tap")
        .args(SUITES)
        .output()
        .unwrap_or_else(|error| {
            panic!("could not run node, which the page module suites need: {error}")
        });
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "the page module suites failed:\n{text}"
    );
    let count = |key: &str| -> usize {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("# {key} ")))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or_else(|| panic!("node reported no `# {key}` count:\n{text}"))
    };
    assert_eq!(
        count("fail"),
        0,
        "the page module suites reported failures:\n{text}"
    );
    let passed = count("pass");
    assert!(
        passed >= COLLECTED_FLOOR,
        "the page module suites passed {passed} tests, under the {COLLECTED_FLOOR} they define:\n{text}"
    );
}

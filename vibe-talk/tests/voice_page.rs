//! Runs the app page's own test suite (the page served at `/`) as part of `cargo test`.
//!
//! The page is plain browser JavaScript with no build step, so its behaviour cannot be asserted
//! from Rust directly — but it is also the surface the owner touches, and it had a real bug
//! (errors visible only in the dev console). A suite that only runs when somebody remembers to run
//! it is a suite that goes stale, so it hangs off the same `cargo test` everything else does.
//!
//! `tests/js/voice_page.test.mjs` executes `web/voice.js` against a small strict DOM; see its
//! header for what that fixture can and cannot fake.
//!
//! The suite is thousands of tests that node runs one after another, so this runs it as several
//! interleaved slices side by side (`VOICE_PAGE_SHARD` in that file; `test-time-vibe-talk`), as
//! many as `VOICE_PAGE_JOBS` says, or one per core up to eight. The validation graph runs the same
//! slices as steps of its own, straight through node, so that they need no test binary built.
//!
//! # This does not skip
//!
//! If Node is missing, this FAILS. A test that turns itself off when a tool is absent reports
//! green for a suite that never ran, and the page's error handling is exactly the kind of thing
//! that would then rot unnoticed.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;

const SUITE: &str = "tests/js/voice_page.test.mjs";

/// How many slices run at once: `VOICE_PAGE_JOBS`, or one per core up to eight.
fn slices() -> usize {
    match std::env::var("VOICE_PAGE_JOBS") {
        Ok(value) => value
            .parse()
            .ok()
            .filter(|n: &usize| *n >= 1)
            .unwrap_or_else(|| {
                panic!("VOICE_PAGE_JOBS must be a whole number of at least 1, not {value:?}")
            }),
        Err(_) => thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8),
    }
}

/// One slice's run of node, stdout and stderr together.
fn run_slice(root: &PathBuf, index: usize, count: usize) -> (Output, String) {
    let output = Command::new("node")
        .current_dir(root)
        // Held to what a slice keeps alive: on a host with a lot of memory V8 otherwise lets
        // garbage pile up to gigabytes a slice, and these run side by side.
        .arg("--max-old-space-size=1024")
        // Importing `node:test` runs the suite and emits TAP. Calling the file directly works on
        // Node 16 too; its experimental `--test` mode otherwise reports the whole file as one test.
        .arg(SUITE)
        .env("VOICE_PAGE_SHARD", format!("{index}/{count}"))
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "could not run node, which {SUITE} needs: {error}. Install Node (any recent \
                 version; no packages, no lockfile) and re-run. This test does not skip: a \
                 skipped page suite reads as green while the page is untested."
            )
        });
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output, text)
}

#[test]
fn the_voice_page_suite_passes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let count = slices();
    let runs: Vec<(Output, String)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..count)
            .map(|index| {
                let root = &root;
                scope.spawn(move || run_slice(root, index, count))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a slice's thread panicked"))
            .collect()
    });

    let mut passed = 0;
    let mut selected = 0;
    let mut defined = None;
    for (index, (output, text)) in runs.iter().enumerate() {
        // Counts, not just the exit code: a suite whose file failed to load exits nonzero, but a
        // suite that silently collected NOTHING would exit zero with `# pass 0`.
        let count_of = |label: &str| -> Option<u32> {
            text.lines()
                .filter_map(|line| line.trim_start().strip_prefix(&format!("# {label} ")))
                .filter_map(|n| n.trim().parse().ok())
                .max()
        };
        let slice = format!("slice {index}/{count}");
        let slice_passed = count_of("pass")
            .unwrap_or_else(|| panic!("node reported no pass count for {slice}:\n{text}"));
        let failed = count_of("fail")
            .unwrap_or_else(|| panic!("node reported no fail count for {slice}:\n{text}"));
        assert!(
            output.status.success(),
            "the app page suite failed in {slice}:\n{text}"
        );
        assert_eq!(failed, 0, "the app page suite failed in {slice}:\n{text}");
        passed += slice_passed;

        // `# voice_page shard i/n: X of Y tests`, which the file prints at exit: the slices
        // between them must select every test it defines, each once.
        let (mine, of) = text
            .lines()
            .find_map(|line| {
                let rest = line.strip_prefix(&format!("# voice_page shard {index}/{count}: "))?;
                let (mine, rest) = rest.split_once(" of ")?;
                Some((
                    mine.parse::<u32>().ok()?,
                    rest.strip_suffix(" tests")?.parse::<u32>().ok()?,
                ))
            })
            .unwrap_or_else(|| panic!("{slice} did not say which tests it selected:\n{text}"));
        assert_eq!(
            *defined.get_or_insert(of),
            of,
            "the slices disagree on how many tests the file defines"
        );
        selected += mine;
    }
    let defined = defined.expect("at least one slice ran");
    assert_eq!(
        selected, defined,
        "the {count} slices selected {selected} of the {defined} tests the file defines"
    );
    // Every selected test reported a pass. The floor against a suite that stopped COLLECTING is
    // `COLLECTED_FLOOR` in the file itself, which fails a slice at exit, so the validation graph's
    // slices, run straight through node, are held to it too.
    assert_eq!(
        passed, selected,
        "the slices passed {passed} of the {selected} tests they selected"
    );
}
